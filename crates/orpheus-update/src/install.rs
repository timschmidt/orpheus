//! The installer: writes an `.ofw` image into the inactive slot through a
//! [`Flash`], in the order the crate documentation's safety argument
//! needs, checking every address against the [`Target`] before the driver
//! (which has its own guard) sees it.
//!
//! Driven step by step so the firmware never blocks for long: [`Installer::start`]
//! (erase the target's header sector), [`Installer::write`] for each piece of
//! the image in order (any size; repeats of the last piece are accepted
//! after a lost acknowledgement), [`Installer::verify_step`] until it returns
//! `true` (read back the whole image, SHA-256 against the header), then
//! [`Installer::commit`].

use orpheus_proto::hash::sum32;
use orpheus_proto::layout::{PAGE, SECTOR};
use orpheus_proto::slot::{HEADER_SIZE, SlotHeader};
use sha2::{Digest, Sha256};

use crate::Error;
use crate::ofw::{self, Header};
use crate::slots::{SUPERSEDED, Target};

/// NOR access the installer needs. The firmware's implementation is the
/// HAL's guarded driver; the host tests use [`crate::sim::SimNor`].
pub trait Flash {
    type Error;
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
    /// Erase the 4 KiB sector at `addr` (aligned).
    fn erase_sector(&mut self, addr: u32) -> Result<(), Self::Error>;
    /// Program `data` (≤ 256 bytes, within one 256-byte page) at `addr`.
    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Checked, nothing written yet.
    Ready,
    /// Target header sector erased; image arriving.
    Writing,
    /// All data written and its SHA-256 right; reading back.
    Verifying,
    /// Read-back verified; [`Installer::commit`] makes it the boot slot.
    Commit,
    /// The new image starts at the next boot.
    Done,
    /// Stopped; the target slot is not valid, the running one untouched.
    Failed(Error),
}

pub struct Installer {
    target: Target,
    header: Header,
    slot: [u8; HEADER_SIZE],
    res_off: u32,
    res_end: u32,
    res_sum: u32,
    received: u32,
    /// Offset up to which the target's sectors are erased.
    erased_to: u32,
    sum: u32,
    hash: Sha256,
    vpos: u32,
    phase: Phase,
}

impl Installer {
    /// Check an update (its `.ofw` header and the image's first 128 bytes)
    /// for `target`. Writes nothing.
    pub fn new(ofw_header: &[u8], slot_header: &[u8], target: Target) -> Result<Installer, Error> {
        let (header, s) = ofw::check(ofw_header, slot_header)?;
        if header.image_len > target.size || !target.allows(target.base, header.image_len) {
            return Err(Error::Refused);
        }
        let mut slot = [0u8; HEADER_SIZE];
        slot.copy_from_slice(&slot_header[..HEADER_SIZE]);
        Ok(Installer {
            target,
            header,
            slot,
            res_off: s.resident_off,
            res_end: s.resident_off + s.resident_len,
            res_sum: s.resident_sum,
            received: 0,
            erased_to: SECTOR,
            sum: 0,
            hash: Sha256::new(),
            vpos: 0,
            phase: Phase::Ready,
        })
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }
    #[must_use]
    pub const fn target(&self) -> Target {
        self.target
    }
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }
    /// Image bytes received so far.
    #[must_use]
    pub const fn received(&self) -> u32 {
        self.received
    }
    /// Image bytes read back and hashed so far.
    #[must_use]
    pub const fn verified(&self) -> u32 {
        self.vpos
    }
    #[must_use]
    pub const fn image_len(&self) -> u32 {
        self.header.image_len
    }

    /// Progress of the whole update in per mille: writing is the first
    /// 80 %, the read-back the rest.
    #[must_use]
    pub fn permille(&self) -> u16 {
        let len = u64::from(self.header.image_len.max(1));
        let p = match self.phase {
            Phase::Ready | Phase::Failed(_) => 0,
            Phase::Writing => u64::from(self.received) * 800 / len,
            Phase::Verifying => 800 + u64::from(self.vpos) * 200 / len,
            Phase::Commit | Phase::Done => 1000,
        };
        p as u16
    }

    fn fail(&mut self, e: Error) -> Error {
        self.phase = Phase::Failed(e);
        e
    }

    /// Stop: the target stays invalid (its header sector is erased or not
    /// yet programmed); the running slot is untouched.
    pub fn abort(&mut self) {
        if !matches!(self.phase, Phase::Done) {
            self.phase = Phase::Failed(Error::Aborted);
        }
    }

    fn guard(&self, addr: u32, len: u32) -> Result<(), Error> {
        if self.target.allows(addr, len) { Ok(()) } else { Err(Error::Refused) }
    }

    fn erase<F: Flash>(&self, f: &mut F, off: u32) -> Result<(), Error> {
        let a = self.target.base + off;
        self.guard(a, SECTOR)?;
        f.erase_sector(a).map_err(|_| Error::Flash)
    }

    /// Program `data` at slot offset `off` (within one page) and read it back.
    fn program<F: Flash>(&self, f: &mut F, off: u32, data: &[u8]) -> Result<(), Error> {
        let a = self.target.base + off;
        self.guard(a, data.len() as u32)?;
        f.program(a, data).map_err(|_| Error::Flash)?;
        let mut back = [0u8; PAGE as usize];
        let back = &mut back[..data.len()];
        f.read(a, back).map_err(|_| Error::Flash)?;
        if back != data { Err(Error::Verify) } else { Ok(()) }
    }

    /// Erase the target's header sector: from here on stage 2 skips the
    /// target until [`Installer::commit`] finishes.
    pub fn start<F: Flash>(&mut self, f: &mut F) -> Result<(), Error> {
        if self.phase != Phase::Ready {
            return Err(Error::State);
        }
        self.phase = Phase::Writing;
        self.erase(f, 0).map_err(|e| self.fail(e))
    }

    /// The image byte expected at `off` in the header sector.
    fn header_byte(&self, off: u32) -> u8 {
        self.slot.get(off as usize).copied().unwrap_or(0xFF)
    }

    /// The image bytes `data` at offset `off`. Pieces must come in order;
    /// a piece that was already taken (a host repeating after a lost
    /// acknowledgement) is compared with what was written instead.
    pub fn write<F: Flash>(&mut self, f: &mut F, off: u32, data: &[u8]) -> Result<(), Error> {
        if self.phase != Phase::Writing {
            return Err(Error::State);
        }
        let end =
            off.checked_add(data.len() as u32).filter(|&e| e <= self.header.image_len).ok_or(Error::Order)?;
        if off < self.received {
            if end > self.received {
                return Err(Error::Order);
            }
            return self.same(f, off, data).map_err(|e| self.fail(e));
        }
        if off > self.received {
            return Err(Error::Order);
        }
        self.take(f, off, data).map_err(|e| self.fail(e))
    }

    fn same<F: Flash>(&self, f: &mut F, off: u32, data: &[u8]) -> Result<(), Error> {
        let mut back = [0u8; PAGE as usize];
        for (i, piece) in data.chunks(PAGE as usize).enumerate() {
            let at = off + (i as u32) * PAGE;
            let b = &mut back[..piece.len()];
            if at >= SECTOR {
                f.read(self.target.base + at, b).map_err(|_| Error::Flash)?;
            } else {
                for (j, x) in b.iter_mut().enumerate() {
                    *x = if at + (j as u32) < SECTOR { self.header_byte(at + j as u32) } else { 0 };
                }
                if at + piece.len() as u32 > SECTOR {
                    let k = (SECTOR - at) as usize;
                    f.read(self.target.base + SECTOR, &mut b[k..]).map_err(|_| Error::Flash)?;
                }
            }
            if b != piece {
                return Err(Error::Verify);
            }
        }
        Ok(())
    }

    fn take<F: Flash>(&mut self, f: &mut F, off: u32, data: &[u8]) -> Result<(), Error> {
        self.hash.update(data);
        let mut at = off;
        let mut rest = data;
        while !rest.is_empty() {
            if at < SECTOR {
                // The header sector is kept in RAM and programmed last.
                let n = ((SECTOR - at) as usize).min(rest.len());
                for (i, &b) in rest[..n].iter().enumerate() {
                    let o = at + i as u32;
                    if b != self.header_byte(o) {
                        return Err(if (o as usize) < HEADER_SIZE {
                            Error::SlotHeader
                        } else {
                            Error::Padding
                        });
                    }
                }
                at += n as u32;
                rest = &rest[n..];
                continue;
            }
            let n = (((at | (PAGE - 1)) + 1 - at) as usize).min(rest.len());
            let piece = &rest[..n];
            // Resident sum (what stage 2 checks), over the overlap.
            let lo = at.max(self.res_off);
            let hi = (at + n as u32).min(self.res_end);
            if lo < hi {
                self.sum = self.sum.wrapping_add(sum32(&piece[(lo - at) as usize..(hi - at) as usize]));
            }
            while self.erased_to < at + n as u32 {
                self.erase(f, self.erased_to)?;
                self.erased_to += SECTOR;
            }
            if piece.iter().any(|&b| b != 0xFF) {
                self.program(f, at, piece)?;
            }
            at += n as u32;
            rest = &rest[n..];
        }
        self.received = off + data.len() as u32;
        if self.received == self.header.image_len {
            let digest: [u8; 32] = core::mem::take(&mut self.hash).finalize().into();
            if digest != self.header.sha256 {
                return Err(Error::Hash);
            }
            if self.sum != self.res_sum {
                return Err(Error::Sum);
            }
            // Read-back: the header sector as it will be, then the body
            // from the chip.
            self.hash.update(self.slot);
            let ff = [0xFFu8; 64];
            let mut left = SECTOR as usize - HEADER_SIZE;
            while left > 0 {
                let k = left.min(64);
                self.hash.update(&ff[..k]);
                left -= k;
            }
            self.vpos = SECTOR;
            self.phase = Phase::Verifying;
        }
        Ok(())
    }

    /// Read back the next `buf.len()` bytes (at most) and hash them;
    /// `Ok(true)` once the whole image matched the header's SHA-256.
    pub fn verify_step<F: Flash>(&mut self, f: &mut F, buf: &mut [u8]) -> Result<bool, Error> {
        if self.phase != Phase::Verifying {
            return Err(Error::State);
        }
        let n = (self.header.image_len - self.vpos).min(buf.len() as u32) as usize;
        if f.read(self.target.base + self.vpos, &mut buf[..n]).is_err() {
            return Err(self.fail(Error::Flash));
        }
        self.hash.update(&buf[..n]);
        self.vpos += n as u32;
        if self.vpos < self.header.image_len {
            return Ok(false);
        }
        let digest: [u8; 32] = core::mem::take(&mut self.hash).finalize().into();
        if digest != self.header.sha256 {
            return Err(self.fail(Error::Hash));
        }
        self.phase = Phase::Commit;
        Ok(true)
    }

    /// Make the new image the one stage 2 starts: header without its magic
    /// word, then the magic word, then (running A, target B) supersede the
    /// running slot.
    pub fn commit<F: Flash>(&mut self, f: &mut F) -> Result<(), Error> {
        if self.phase != Phase::Commit {
            return Err(Error::State);
        }
        let r = self.commit_inner(f);
        match r {
            Ok(()) => self.phase = Phase::Done,
            Err(e) => {
                self.fail(e);
            }
        }
        r
    }

    fn commit_inner<F: Flash>(&mut self, f: &mut F) -> Result<(), Error> {
        let base = self.target.base;
        // The header sector must still be erased past the header.
        let mut back = [0u8; PAGE as usize];
        let mut at = 0;
        while at < SECTOR {
            f.read(base + at, &mut back).map_err(|_| Error::Flash)?;
            if back.iter().any(|&b| b != 0xFF) {
                return Err(Error::Verify);
            }
            at += PAGE;
        }
        let slot = self.slot;
        self.program(f, 4, &slot[4..])?;
        self.program(f, 0, &slot[..4])?;
        let mut h = [0u8; HEADER_SIZE];
        f.read(base, &mut h).map_err(|_| Error::Flash)?;
        if h != slot || SlotHeader::from_bytes(&h).is_err() {
            return Err(Error::Verify);
        }
        if let Some(old) = self.target.supersede {
            self.guard(old, 4)?;
            f.program(old, &SUPERSEDED.to_le_bytes()).map_err(|_| Error::Flash)?;
            let mut w = [0u8; 4];
            f.read(old, &mut w).map_err(|_| Error::Flash)?;
            if u32::from_le_bytes(w) != SUPERSEDED {
                return Err(Error::Verify);
            }
        }
        Ok(())
    }

    /// Run a whole update over in-memory data (host tools, tests).
    pub fn run<F: Flash>(&mut self, f: &mut F, image: &[u8], chunk: usize) -> Result<(), Error> {
        self.start(f)?;
        for (i, c) in image.chunks(chunk.max(1)).enumerate() {
            self.write(f, (i * chunk.max(1)) as u32, c)?;
        }
        let mut buf = [0u8; 1024];
        while !self.verify_step(f, &mut buf)? {}
        self.commit(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofw::tests::{image, ofw_header};
    use crate::sim::SimNor;
    use crate::slots::{self, SlotState, SwitchStep, boots, choose, stage2_pick, state_of, switch};
    use orpheus_proto::layout::{SLOTS, full};
    use std::vec::Vec;

    fn installed(nor: &mut SimNor, slot: u32, img: &[u8]) {
        nor.load(slot, img);
    }

    fn pick(nor: &SimNor) -> Option<usize> {
        let mut r = nor.reader();
        stage2_pick(&mut r)
    }

    fn boots_ok(nor: &SimNor, i: usize) -> bool {
        let mut r = nor.reader();
        boots(i, &mut r)
    }

    /// The image stage 2 starts, compared with `img` over its extent.
    fn holds(nor: &SimNor, i: usize, img: &[u8]) -> bool {
        let b = SLOTS[i] as usize;
        nor.data[b..b + img.len()] == *img
    }

    fn update(nor: &mut SimNor, booted: u8, img: &[u8], chunk: usize) -> Result<(), Error> {
        let t = choose(true, Some(booted))?;
        let h = ofw_header(img, "2.0", "");
        let mut i = Installer::new(&h, &img[..128], t)?;
        i.run(nor, img, chunk)
    }

    #[test]
    fn a_to_b_and_back() {
        let old = image(1, 0x2345, 2);
        let new = image(2, 0x3456, 3);
        let mut nor = SimNor::new();
        installed(&mut nor, full::SLOT_A, &old);
        assert_eq!(pick(&nor), Some(0));
        update(&mut nor, 0, &new, 1024).unwrap();
        assert_eq!(pick(&nor), Some(1));
        assert!(boots_ok(&nor, 1) && holds(&nor, 1, &new));
        // A was superseded, not erased: it can be switched back to.
        let mut ha = [0u8; 128];
        ha.copy_from_slice(&nor.data[0x1000..0x1080]);
        assert!(matches!(state_of(&ha), SlotState::Superseded(_)));
        // And the next update goes to A, which then starts first.
        let third = image(3, 0x1111, 1);
        update(&mut nor, 1, &third, 4096).unwrap();
        assert_eq!(pick(&nor), Some(0));
        assert!(boots_ok(&nor, 0) && holds(&nor, 0, &third));
        assert_eq!((nor.forbidden, nor.unerased), (0, 0));
    }

    #[test]
    fn any_chunk_size_and_repeats() {
        let old = image(1, 0x2000, 1);
        let new = image(5, 0x2F01, 2);
        for chunk in [1usize, 100, 256, 777, 1024, 4096, 65536] {
            let mut nor = SimNor::new();
            installed(&mut nor, full::SLOT_A, &old);
            update(&mut nor, 0, &new, chunk).unwrap();
            assert!(boots_ok(&nor, 1) && holds(&nor, 1, &new), "chunk {chunk}");
        }
        // A repeated piece is accepted if it matches; overlapping, skipped
        // or changed pieces are not.
        let mut nor = SimNor::new();
        installed(&mut nor, full::SLOT_A, &old);
        let h = ofw_header(&new, "2.0", "");
        let mut i = Installer::new(&h, &new[..128], choose(true, Some(0)).unwrap()).unwrap();
        assert_eq!(i.write(&mut nor, 0, &new[..1024]), Err(Error::State));
        i.start(&mut nor).unwrap();
        for k in 0..6 {
            i.write(&mut nor, k * 1024, &new[k as usize * 1024..(k as usize + 1) * 1024]).unwrap();
            i.write(&mut nor, k * 1024, &new[k as usize * 1024..(k as usize + 1) * 1024]).unwrap();
        }
        assert_eq!(i.write(&mut nor, 7 * 1024, &new[7 * 1024..8 * 1024]), Err(Error::Order));
        assert_eq!(i.write(&mut nor, 5 * 1024, &new[5 * 1024..7 * 1024]), Err(Error::Order));
        assert_eq!(i.write(&mut nor, u32::MAX, &new[..2]), Err(Error::Order));
        let mut changed = new[5 * 1024..6 * 1024].to_vec();
        changed[7] ^= 1;
        assert_eq!(i.write(&mut nor, 5 * 1024, &changed), Err(Error::Verify));
        assert_eq!(i.phase(), Phase::Failed(Error::Verify));
        assert_eq!(pick(&nor), Some(0));
    }

    #[test]
    fn damaged_data_keeps_the_old_slot() {
        let old = image(1, 0x2000, 1);
        let new = image(6, 0x2800, 2);
        // A flipped byte in the body, in the header, in the padding.
        for (at, want) in [(0x5000usize, Error::Hash), (0x20, Error::SlotHeader), (0x900, Error::Padding)] {
            let mut nor = SimNor::new();
            installed(&mut nor, full::SLOT_A, &old);
            let h = ofw_header(&new, "2.0", "");
            let mut bad = new.clone();
            bad[at] ^= 0x40;
            let mut i = Installer::new(&h, &new[..128], choose(true, Some(0)).unwrap()).unwrap();
            assert_eq!(i.run(&mut nor, &bad, 1024), Err(want));
            assert_eq!(pick(&nor), Some(0));
            assert!(holds(&nor, 0, &old));
            assert_eq!(state_of(&nor.data[0x200000..0x200080]), SlotState::Empty);
        }
        // The resident sum in the slot header disagrees with the data (a
        // header built for another resident image, hash made to match).
        let mut nor = SimNor::new();
        installed(&mut nor, full::SLOT_A, &old);
        let mut s = SlotHeader::from_bytes(&new).unwrap();
        s.resident_sum ^= 1;
        let mut bad = new.clone();
        bad[..128].copy_from_slice(&s.to_bytes());
        let h = ofw_header(&bad, "2.0", "");
        let mut i = Installer::new(&h, &bad[..128], choose(true, Some(0)).unwrap()).unwrap();
        assert_eq!(i.run(&mut nor, &bad, 1024), Err(Error::Sum));
        assert_eq!(pick(&nor), Some(0));
    }

    #[test]
    fn read_back_catches_a_chip_that_lost_a_bit() {
        let old = image(1, 0x2000, 1);
        let new = image(7, 0x2800, 2);
        let mut nor = SimNor::new();
        installed(&mut nor, full::SLOT_A, &old);
        let h = ofw_header(&new, "2.0", "");
        let mut i = Installer::new(&h, &new[..128], choose(true, Some(0)).unwrap()).unwrap();
        i.start(&mut nor).unwrap();
        for (k, c) in new.chunks(1024).enumerate() {
            i.write(&mut nor, (k * 1024) as u32, c).unwrap();
        }
        assert_eq!(i.phase(), Phase::Verifying);
        nor.data[0x200000 + 0x3003] ^= 0x01;
        let mut buf = [0u8; 512];
        let r = loop {
            match i.verify_step(&mut nor, &mut buf) {
                Ok(true) => break Ok(()),
                Ok(false) => {}
                Err(e) => break Err(e),
            }
        };
        assert_eq!(r, Err(Error::Hash));
        assert_eq!(i.commit(&mut nor), Err(Error::State));
        assert_eq!(pick(&nor), Some(0));
    }

    #[test]
    fn guards_refuse_sector0_and_vendor_data() {
        let new = image(8, 0x2000, 1);
        let h = ofw_header(&new, "2.0", "");
        // Forged targets: refused before anything is written.
        for t in [
            slots::Target { index: 0, base: 0, size: 0x10_0000, supersede: None },
            slots::Target { index: 0, base: 0x7E0000, size: 0x10_0000, supersede: None },
            slots::Target { index: 0, base: 0x7D0000, size: 0x10_0000, supersede: None },
            slots::Target { index: 1, base: full::SLOT_B, size: 0x1000, supersede: None },
        ] {
            assert_eq!(Installer::new(&h, &new[..128], t).err(), Some(Error::Refused), "{t:?}");
        }
        // A target whose supersede word is sector 0: the commit refuses
        // that write (and the sim chip would count it).
        let t = slots::Target { index: 1, base: full::SLOT_B, size: 0x1F_F000, supersede: Some(0) };
        let mut nor = SimNor::new();
        let mut i = Installer::new(&h, &new[..128], t).unwrap();
        assert_eq!(i.run(&mut nor, &new, 4096), Err(Error::Refused));
        assert_eq!(nor.data[..0x1000], [0xFFu8; 0x1000]);
        assert_eq!(nor.forbidden, 0);
        // The chip model itself refuses (and counts) such writes.
        assert!(nor.erase_sector(0).is_err());
        assert!(nor.program(0x7E0000, &[0]).is_err());
        assert!(nor.program(0xFFF, &[0, 0]).is_err());
        assert_eq!(nor.forbidden, 3);
        // An image larger than a slot is refused by the format check.
        let mut big = Header::parse(&h).unwrap();
        big.image_len = 0x20_0000;
        assert_eq!(
            Installer::new(&big.to_bytes(""), &new[..128], choose(true, Some(0)).unwrap()).err(),
            Some(Error::Size)
        );
    }

    /// Run the update with the power cut before operation `cut`; returns
    /// whether it was cut.
    fn cut_run(nor: &mut SimNor, booted: u8, img: &[u8], cut: usize, seed: u32) -> bool {
        nor.cut_after = Some(nor.ops + cut);
        nor.seed = seed;
        let r = update(nor, booted, img, 1024);
        let cut_happened = nor.dead;
        nor.power_on();
        assert!(r.is_ok() || cut_happened, "{r:?}");
        cut_happened
    }

    /// Every intermediate state boots the old or the new image, and a
    /// second attempt after the cut always succeeds.
    fn power_loss_everywhere(old_slot: u32, booted: u8) {
        let old = image(11, 0x2345, 2);
        let new = image(12, 0x1F00, 3);
        let mut clean = SimNor::new();
        installed(&mut clean, old_slot, &old);
        let base_ops = clean.ops;
        update(&mut clean, booted, &new, 1024).unwrap();
        let total = clean.ops - base_ops;
        let new_index = if booted == 0 { 1 } else { 0 };
        let mut cuts = 0;
        for cut in 0..=total {
            for seed in [1u32, 0x9E37_79B9, 0x1234_5678] {
                let mut nor = SimNor::new();
                installed(&mut nor, old_slot, &old);
                if !cut_run(&mut nor, booted, &new, cut, seed) {
                    continue;
                }
                cuts += 1;
                let p = pick(&nor).expect("a power cut left nothing bootable");
                let old_index = SLOTS.iter().position(|&b| b == old_slot).unwrap();
                assert!(
                    (p == old_index && holds(&nor, p, &old)) || (p == new_index && holds(&nor, p, &new)),
                    "cut {cut} seed {seed:#x}: stage 2 picks slot {p} with neither image"
                );
                assert!(boots_ok(&nor, p), "cut {cut}: header CRC");
                // The next attempt (from whatever booted) completes.
                let again = if p == new_index {
                    // Already the new image: an update from it writes the
                    // other slot; nothing to redo.
                    continue;
                } else {
                    booted
                };
                update(&mut nor, again, &new, 4096).unwrap();
                assert_eq!(pick(&nor), Some(new_index), "cut {cut}");
                assert!(holds(&nor, new_index, &new));
                assert_eq!(nor.forbidden, 0);
            }
        }
        assert!(cuts > 3 * total / 2, "{cuts} cuts of {total} operations");
    }

    #[test]
    fn power_loss_a_to_b() {
        power_loss_everywhere(full::SLOT_A, 0);
    }

    #[test]
    fn power_loss_b_to_a() {
        // B runs, A holds a superseded older image.
        power_loss_everywhere(full::SLOT_B, 1);
    }

    #[test]
    fn power_loss_from_the_old_dual_slot() {
        power_loss_everywhere(orpheus_proto::layout::dual::SLOT_A, 2);
    }

    #[test]
    fn switch_back_and_forth() {
        let a = image(21, 0x2000, 1);
        let b = image(22, 0x2100, 1);
        let mut nor = SimNor::new();
        installed(&mut nor, full::SLOT_A, &a);
        update(&mut nor, 0, &b, 1024).unwrap();
        assert_eq!(pick(&nor), Some(1));
        // B crashes (boot counter → USB recovery): the host switches back.
        for want in [0, 1, 0, 1] {
            let (ha, hb) = (nor.data[0x1000..0x1080].to_vec(), nor.data[0x200000..0x200080].to_vec());
            let steps = switch(&ha, &hb).unwrap();
            for s in steps.iter().flatten() {
                apply(&mut nor, s);
                // After every step one complete image boots.
                let p = pick(&nor).unwrap();
                assert!(holds(&nor, p, if p == 0 { &a } else { &b }));
            }
            assert_eq!(pick(&nor), Some(want));
            assert!(boots_ok(&nor, want));
        }
        assert_eq!(nor.forbidden, 0);
        // Nothing to switch to.
        assert!(switch(&[0xFF; 128], &[0xFF; 128]).is_none());
        // A runs and B is empty: nothing to switch to; A superseded and B
        // empty: restoring A is the way back.
        let valid_a = slots::restored(&nor.data[0x1000..0x1080]).unwrap();
        assert!(switch(&valid_a, &[0xFF; 128]).is_none());
        assert!(matches!(
            switch(&nor.data[0x1000..0x1080], &[0xFF; 128]),
            Some([Some(SwitchStep::Restore { slot: 0x1000, .. }), None])
        ));
        // A damaged superseded header is not restored.
        let mut ha = nor.data[0x1000..0x1080].to_vec();
        ha[..4].fill(0);
        ha[0x30] ^= 1;
        assert_eq!(state_of(&ha), SlotState::Empty);
    }

    fn apply(nor: &mut SimNor, s: &SwitchStep) {
        match *s {
            SwitchStep::Restore { slot, header } => {
                nor.erase_sector(slot).unwrap();
                nor.program(slot, &header).unwrap();
            }
            SwitchStep::Supersede { slot } => nor.program(slot, &0u32.to_le_bytes()).unwrap(),
        }
    }

    #[test]
    fn progress() {
        let new = image(9, 0x2000, 1);
        let h = ofw_header(&new, "2.0", "");
        let mut nor = SimNor::new();
        let mut i = Installer::new(&h, &new[..128], choose(true, Some(1)).unwrap()).unwrap();
        assert_eq!(i.permille(), 0);
        i.start(&mut nor).unwrap();
        let half = new.len() / 2 / 1024 * 1024;
        i.write(&mut nor, 0, &new[..half]).unwrap();
        assert!((300..500).contains(&i.permille()));
        i.write(&mut nor, half as u32, &new[half..]).unwrap();
        assert_eq!(u64::from(i.permille()), 800 + 0x1000 * 200 / new.len() as u64);
        let mut buf: Vec<u8> = std::vec![0; 4096];
        while !i.verify_step(&mut nor, &mut buf).unwrap() {}
        i.commit(&mut nor).unwrap();
        assert_eq!((i.permille(), i.phase()), (1000, Phase::Done));
        i.abort();
        assert_eq!(i.phase(), Phase::Done);
    }
}
