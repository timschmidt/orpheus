//! MMC commands of a CD/DVD-ROM unit (06-connectivity §3.3): one data
//! track (mode 1, 2048-byte blocks) in one closed session, as an ISO image
//! presents itself. Images larger than a CD ([`CD_MAX_BLOCKS`]) report the
//! DVD-ROM profile.
//!
//! Field layouts follow MMC-5 (T10/1675D); the values mirror what pressed
//! single-track discs return in common drives. Everything not handled
//! here ends as ILLEGAL REQUEST / INVALID COMMAND OPERATION CODE.

use super::scsi::{DeviceKind, Plan, Scsi, Sense, Unit, be16, be32};

/// 2048-byte blocks on an 80-minute CD (360 000 sectors), rounded down to
/// 700 MiB: above this an image is presented as a DVD.
pub const CD_MAX_BLOCKS: u64 = 700 * 1024 * 1024 / 2048;
/// Logical block size of the unit.
pub const CD_BLOCK: u32 = 2048;

/// MMC profile numbers.
pub const PROFILE_CD_ROM: u16 = 0x0008;
pub const PROFILE_DVD_ROM: u16 = 0x0010;

/// ADR 1 (position), control 4 (data track, recorded uninterrupted).
const ADR_CTL_DATA: u8 = 0x14;
const LEAD_OUT: u8 = 0xAA;

impl Unit {
    /// A CD-ROM unit for an image of `bytes` bytes (the tail of an image
    /// whose size is not a multiple of 2048 is padded by the block device).
    #[must_use]
    pub fn cdrom(bytes: u64, vendor: [u8; 8], product: [u8; 16], revision: [u8; 4]) -> Self {
        Unit {
            kind: DeviceKind::CdRom,
            removable: true,
            block_size: CD_BLOCK,
            blocks: bytes.div_ceil(u64::from(CD_BLOCK)),
            read_only: true,
            present: true,
            vendor,
            product,
            revision,
        }
    }

    /// The unit presents the DVD-ROM profile.
    #[must_use]
    pub fn is_dvd(&self) -> bool {
        self.kind == DeviceKind::CdRom && self.blocks > CD_MAX_BLOCKS
    }

    fn profile(&self) -> u16 {
        if !self.present {
            0
        } else if self.is_dvd() {
            PROFILE_DVD_ROM
        } else {
            PROFILE_CD_ROM
        }
    }
}

/// Commands answered whether or not a medium is present, and without
/// consuming a unit attention (MMC: GET CONFIGURATION and GET EVENT STATUS
/// NOTIFICATION never report one; the others are drive, not medium,
/// queries).
#[must_use]
pub fn medium_independent(op: u8) -> bool {
    matches!(op, 0x46 | 0x4A | 0xBD | 0x1A | 0x5A | 0xBB)
}

/// LBA to minutes/seconds/frames (75 frames per second, 2 s pre-gap),
/// saturating where a DVD-sized address no longer fits.
#[must_use]
pub fn lba_to_msf(lba: u32) -> [u8; 3] {
    let a = u64::from(lba) + 150;
    let m = a / (75 * 60);
    if m > 255 {
        return [255, 59, 74];
    }
    [m as u8, (a / 75 % 60) as u8, (a % 75) as u8]
}

fn addr(lba: u32, msf: bool) -> [u8; 4] {
    if msf {
        let [m, s, f] = lba_to_msf(lba);
        [0, m, s, f]
    } else {
        lba.to_be_bytes()
    }
}

impl Scsi {
    /// Interpret an MMC command for a CD/DVD unit.
    pub(super) fn mmc(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        match cdb[0] {
            0x46 => self.get_configuration(u, cdb),
            0x4A => self.event_status(u, cdb),
            0xBD => {
                // MECHANISM STATUS: no changer, idle, tray closed, no slot
                // tables.
                self.buf[..8].fill(0);
                self.data(8, usize::from(be16(cdb, 8)))
            }
            0x1A | 0x5A => self.mode_sense_cd(u, cdb),
            0xBB => Plan::Done, // SET CD SPEED: accepted, nothing to change
            0x43 => self.read_toc(u, cdb),
            0x44 => {
                // READ HEADER: mode 1 data at the address.
                let lba = be32(cdb, 2);
                if u64::from(lba) >= u.blocks {
                    return self.fail(Sense::LBA_OUT_OF_RANGE);
                }
                self.buf[..4].copy_from_slice(&[1, 0, 0, 0]);
                self.buf[4..8].copy_from_slice(&addr(lba, cdb.get(1).copied().unwrap_or(0) & 2 != 0));
                self.data(8, usize::from(be16(cdb, 7)))
            }
            0x51 => self.disc_information(u, cdb),
            0x52 => self.track_information(u, cdb),
            0xAD => self.dvd_structure(u, cdb),
            0xBE => {
                // READ CD: user data of mode-1 sectors only.
                let sector_type = (cdb.get(1).copied().unwrap_or(0) >> 2) & 7;
                let lba = u64::from(be32(cdb, 2));
                let count = be32(cdb, 5) & 0x00FF_FFFF;
                match (sector_type, cdb.get(9).copied().unwrap_or(0)) {
                    (0 | 2, 0x10) => {}
                    (0 | 2, 0x00) => return Plan::Done,
                    _ => return self.fail(Sense::INVALID_FIELD),
                }
                if lba.checked_add(u64::from(count)).is_none_or(|e| e > u.blocks) {
                    return self.fail(Sense::LBA_OUT_OF_RANGE);
                }
                if count == 0 { Plan::Done } else { Plan::Read { lba, count } }
            }
            _ => self.fail(Sense::INVALID_OPCODE),
        }
    }

    fn get_configuration(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let rt = cdb.get(1).copied().unwrap_or(0) & 3;
        if rt == 3 {
            return self.fail(Sense::INVALID_FIELD);
        }
        let start = be16(cdb, 2);
        let alloc = usize::from(be16(cdb, 7));
        let dvd = u.is_dvd();
        let present = u.present;
        let b = &mut self.buf;
        b.fill(0);
        let profile = u.profile();
        b[6..8].copy_from_slice(&profile.to_be_bytes());
        let mut n = 8;
        // (feature, version, persistent, current, additional data)
        let mut profiles = [0u8; 8];
        let plen = if dvd {
            profiles[..4].copy_from_slice(&[0x00, 0x10, u8::from(present), 0]);
            profiles[4..8].copy_from_slice(&[0x00, 0x08, 0, 0]);
            8
        } else {
            profiles[..4].copy_from_slice(&[0x00, 0x08, u8::from(present), 0]);
            4
        };
        let blocking: u16 = if dvd { 16 } else { 1 };
        let mut random = [0u8; 8];
        random[..4].copy_from_slice(&CD_BLOCK.to_be_bytes());
        random[4..6].copy_from_slice(&blocking.to_be_bytes());
        let features: [(u16, u8, bool, bool, &[u8]); 7] = [
            (0x0000, 0, true, true, &profiles[..plen]),
            // Core: physical interface 8 = USB; no DBE, no INQ2.
            (0x0001, 2, true, true, &[0, 0, 0, 8, 0, 0, 0, 0]),
            // Morphing: polling GESN only, no operational-change events.
            (0x0002, 1, true, true, &[0, 0, 0, 0]),
            // Removable medium: tray, eject, lock.
            (0x0003, 0, true, true, &[0x29, 0, 0, 0]),
            (0x0010, 0, false, present, &random),
            (0x001E, 2, false, present && !dvd, &[0, 0, 0, 0]),
            (0x001F, 1, false, present && dvd, &[0, 0, 0, 0]),
        ];
        for (code, version, persistent, current, data) in features {
            if code == 0x001F && !dvd {
                continue;
            }
            let wanted = match rt {
                0 => code >= start,
                1 => code >= start && current,
                _ => code == start,
            };
            if !wanted {
                continue;
            }
            b[n..n + 2].copy_from_slice(&code.to_be_bytes());
            b[n + 2] = (version << 2) | (u8::from(persistent) << 1) | u8::from(current);
            b[n + 3] = data.len() as u8;
            b[n + 4..n + 4 + data.len()].copy_from_slice(data);
            n += 4 + data.len();
        }
        b[..4].copy_from_slice(&((n - 4) as u32).to_be_bytes());
        self.data(n, alloc)
    }

    fn event_status(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        if cdb.get(1).copied().unwrap_or(0) & 1 == 0 {
            // Asynchronous notification is not supported.
            return self.fail(Sense::INVALID_FIELD);
        }
        let classes = cdb.get(4).copied().unwrap_or(0);
        let alloc = usize::from(be16(cdb, 7));
        if classes & 0x10 == 0 {
            // No event of a requested class: NEA, supported = media.
            self.buf[..4].copy_from_slice(&[0, 2, 0x80, 0x10]);
            return self.data(4, alloc);
        }
        let code = core::mem::take(&mut self.media_event);
        let status = if u.present { 0x02 } else { 0x00 };
        self.buf[..8].copy_from_slice(&[0, 6, 0x04, 0x10, code, status, 0, 0]);
        self.data(8, alloc)
    }

    fn mode_sense_cd(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let ten = cdb[0] == 0x5A;
        let pc = cdb.get(2).copied().unwrap_or(0) >> 6;
        let page = cdb.get(2).copied().unwrap_or(0) & 0x3F;
        let (hdr, alloc) = if ten {
            (8usize, usize::from(be16(cdb, 7)))
        } else {
            (4usize, usize::from(cdb.get(4).copied().unwrap_or(0)))
        };
        if pc == 3 {
            return self.fail(Sense::SAVING_NOT_SUPPORTED);
        }
        if page != 0x2A && page != 0x3F {
            return self.fail(Sense::INVALID_FIELD);
        }
        let b = &mut self.buf;
        b[..hdr + 22].fill(0);
        let p = &mut b[hdr..hdr + 22];
        p[0] = 0x2A;
        p[1] = 0x14;
        if pc != 1 {
            // Changeable values (pc 1) are all zero.
            p[2] = if u.is_dvd() { 0x08 } else { 0x00 }; // DVD-ROM read; CD-ROM read is implied
            p[3] = 0x00; // no writing
            p[4] = 0x00; // no audio play, no multi-session
            p[5] = 0x00; // no CD-DA commands
            p[6] = 0x29 | if self.events.prevent_removal { 0x02 } else { 0 }; // tray, eject, lock (+state)
            let speed: u16 = 40 * 176; // nominal "40x", kB/s (obsolete fields)
            p[8..10].copy_from_slice(&speed.to_be_bytes());
            p[12..14].copy_from_slice(&48u16.to_be_bytes()); // buffer, KiB
            p[14..16].copy_from_slice(&speed.to_be_bytes());
        }
        let n = hdr + 22;
        if ten {
            b[..2].copy_from_slice(&((n - 2) as u16).to_be_bytes());
        } else {
            b[0] = (n - 1) as u8;
        }
        self.data(n, alloc)
    }

    fn read_toc(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let msf = cdb.get(1).copied().unwrap_or(0) & 2 != 0;
        let mut format = cdb.get(2).copied().unwrap_or(0) & 0x0F;
        // Pre-MMC initiators put the format in the top bits of byte 9.
        let old = cdb.get(9).copied().unwrap_or(0) >> 6;
        if format == 0 && old != 0 {
            format = old;
        }
        let track = cdb.get(6).copied().unwrap_or(0);
        let alloc = usize::from(be16(cdb, 7));
        let lead_out = u.blocks.min(u64::from(u32::MAX)) as u32;
        let b = &mut self.buf;
        b.fill(0);
        let n = match format {
            0 => {
                // Formatted TOC: track 1 and the lead-out.
                let mut n = 4;
                let mut put = |b: &mut [u8], t: u8, lba: u32| {
                    b[n + 1] = ADR_CTL_DATA;
                    b[n + 2] = t;
                    b[n + 4..n + 8].copy_from_slice(&addr(lba, msf));
                    n += 8;
                };
                match track {
                    0 | 1 => {
                        put(b, 1, 0);
                        put(b, LEAD_OUT, lead_out);
                    }
                    LEAD_OUT => put(b, LEAD_OUT, lead_out),
                    _ => return self.fail(Sense::INVALID_FIELD),
                }
                n
            }
            1 => {
                // Session information: one complete session from track 1.
                b[4..8].copy_from_slice(&[0, ADR_CTL_DATA, 1, 0]);
                b[8..12].copy_from_slice(&addr(0, msf));
                12
            }
            2 => {
                // Raw (full) TOC, always MSF: A0, A1, A2, track 1.
                let [lm, ls, lf] = lba_to_msf(lead_out);
                let points: [(u8, [u8; 3]); 4] =
                    [(0xA0, [1, 0x00, 0]), (0xA1, [1, 0, 0]), (0xA2, [lm, ls, lf]), (1, [0, 2, 0])];
                let mut n = 4;
                for (point, p) in points {
                    b[n] = 1; // session
                    b[n + 1] = ADR_CTL_DATA;
                    b[n + 3] = point;
                    b[n + 8..n + 11].copy_from_slice(&p);
                    n += 11;
                }
                n
            }
            _ => return self.fail(Sense::INVALID_FIELD),
        };
        b[..2].copy_from_slice(&((n - 2) as u16).to_be_bytes());
        b[2] = 1;
        b[3] = 1;
        self.data(n, alloc)
    }

    fn disc_information(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        if cdb.get(1).copied().unwrap_or(0) & 7 != 0 {
            return self.fail(Sense::INVALID_FIELD);
        }
        let alloc = usize::from(be16(cdb, 7));
        let b = &mut self.buf;
        b[..34].fill(0);
        b[1] = 32; // data length
        b[2] = 0x0E; // not erasable, last session complete, disc complete
        b[3] = 1; // first track on disc
        b[4] = 1; // sessions
        b[5] = 1; // first track in last session
        b[6] = 1; // last track in last session
        b[7] = 0x00; // no disc id / bar code / URU
        b[8] = 0x00; // CD-DA or CD-ROM disc
        b[16..24].fill(0xFF); // no further lead-in / lead-out (complete)
        let _ = u;
        self.data(34, alloc)
    }

    fn track_information(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let kind = cdb.get(1).copied().unwrap_or(0) & 3;
        let v = be32(cdb, 2);
        let ok = match kind {
            0 => u64::from(v) < u.blocks,
            1 | 2 => v == 1,
            _ => false,
        };
        if !ok {
            return self.fail(if kind == 0 { Sense::LBA_OUT_OF_RANGE } else { Sense::INVALID_FIELD });
        }
        let alloc = usize::from(be16(cdb, 7));
        let blocks = u.blocks.min(u64::from(u32::MAX)) as u32;
        let b = &mut self.buf;
        b[..36].fill(0);
        b[1] = 34;
        b[2] = 1; // track
        b[3] = 1; // session
        b[5] = 0x04; // data track, uninterrupted, not copyable
        b[6] = 0x01; // data mode 1; not blank, not packet, not reserved
        b[24..28].copy_from_slice(&blocks.to_be_bytes()); // track size
        b[28..32].copy_from_slice(&blocks.saturating_sub(1).to_be_bytes()); // last recorded
        self.data(36, alloc)
    }

    fn dvd_structure(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let layer = cdb.get(6).copied().unwrap_or(0);
        let format = cdb.get(7).copied().unwrap_or(0);
        let alloc = usize::from(be16(cdb, 8));
        if format != 0 || layer != 0 {
            return self.fail(Sense::INVALID_FIELD);
        }
        if !u.is_dvd() {
            return self.fail(Sense::INCOMPATIBLE_FORMAT);
        }
        // Physical format information (ECMA-267 §3.3.2): 2048 bytes after a
        // 4-byte header; only the first 17 are non-zero.
        const START_PSN: u32 = 0x0003_0000;
        let end = START_PSN + (u.blocks.min(u64::from(u32::MAX - START_PSN)) as u32).saturating_sub(1);
        let b = &mut self.buf;
        b[..21].fill(0);
        b[..2].copy_from_slice(&2050u16.to_be_bytes());
        b[4] = 0x01; // DVD-ROM, part version 1
        b[5] = 0x0F; // 120 mm, maximum rate not specified
        b[6] = 0x01; // one layer, parallel track path, embossed data
        b[7] = 0x00; // densities
        b[8..12].copy_from_slice(&START_PSN.to_be_bytes());
        b[12..16].copy_from_slice(&end.to_be_bytes());
        // Layer-0 end sector 0 (single layer), no BCA. The head is cut to
        // the allocation length too, as the plan promises (VER-4 F-USB-1).
        let len = 2052.min(alloc);
        Plan::DataInPadded { head: &self.buf[..21.min(len)], len }
    }
}

#[cfg(test)]
mod tests {
    //! Scripted initiators. The command sequences are reconstructed from
    //! the initiators' published sources (not captured traces, which need
    //! the device): Linux `drivers/scsi/sr.c` probe (`get_capabilities`,
    //! `sr_read_capacity`) and `drivers/cdrom/cdrom.c` (`cdrom_get_disc_info`,
    //! `cdrom_get_track_info`, `cdrom_read_tocentry`, `cdrom_get_last_written`,
    //! media-event polling through `sr_check_events`), the Windows class
    //! driver (`storage/class/cdrom` in Microsoft's Windows-driver-samples:
    //! GET CONFIGURATION header then full, MODE SENSE(10) page 0x2A, READ
    //! TOC formats 0/1/2 in MSF, GESN polling), and EDK2's
    //! `UsbMassStorageDxe` (INQUIRY, TEST UNIT READY with REQUEST SENSE
    //! until ready, READ CAPACITY(10), READ(10)). Expected bytes are built
    //! from MMC-5 by hand.

    use super::*;
    use crate::msc::scsi::{DeviceKind, Plan, Scsi, Sense};
    use std::vec::Vec;

    fn cd(bytes: u64) -> Unit {
        Unit::cdrom(bytes, *b"orpheus ", *b"USB Drive CD    ", *b"0001")
    }

    fn run(s: &mut Scsi, u: &Unit, cdb: &[u8]) -> Result<Vec<u8>, Sense> {
        match s.command(u, cdb) {
            Plan::DataIn(d) => Ok(d.to_vec()),
            Plan::DataInPadded { head, len } => {
                let mut v = head.to_vec();
                v.resize(len, 0);
                Ok(v)
            }
            Plan::Done => Ok(Vec::new()),
            Plan::Read { lba, count } => Ok([lba.to_be_bytes().as_slice(), &count.to_be_bytes()].concat()),
            Plan::Write { .. } => panic!("write on a CD"),
            Plan::Fail => Err(s.sense()),
        }
    }

    /// A 2-hour-of-nothing example: 1000 blocks.
    const SMALL: u64 = 1000 * 2048;

    #[test]
    fn toc_format0_lba_and_msf() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        // LBA, from track 1, allocation 0x324 (what cdrom.c uses for the
        // whole TOC).
        let t = run(&mut s, &u, &[0x43, 0, 0, 0, 0, 0, 1, 0x03, 0x24, 0]).unwrap();
        assert_eq!(
            t,
            [
                0x00, 0x12, 0x01, 0x01, // length 18, tracks 1..1
                0x00, 0x14, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // track 1 at 0
                0x00, 0x14, 0xAA, 0x00, 0x00, 0x00, 0x03, 0xE8, // lead-out at 1000
            ]
        );
        // MSF: 00:02:00 and 1000+150 = 1150 frames = 00:15:25.
        let t = run(&mut s, &u, &[0x43, 2, 0, 0, 0, 0, 0, 0, 20, 0]).unwrap();
        assert_eq!(&t[4..12], &[0, 0x14, 1, 0, 0, 0, 2, 0]);
        assert_eq!(&t[12..20], &[0, 0x14, 0xAA, 0, 0, 0, 15, 25]);
        // Lead-out only; a track that does not exist.
        let t = run(&mut s, &u, &[0x43, 0, 0, 0, 0, 0, 0xAA, 0, 12, 0]).unwrap();
        assert_eq!(t, [0, 10, 1, 1, 0, 0x14, 0xAA, 0, 0, 0, 0x03, 0xE8]);
        assert_eq!(run(&mut s, &u, &[0x43, 0, 0, 0, 0, 0, 2, 0, 12, 0]), Err(Sense::INVALID_FIELD));
        // Header only (Windows asks for 4 bytes first).
        assert_eq!(run(&mut s, &u, &[0x43, 2, 0, 0, 0, 0, 0, 0, 4, 0]).unwrap(), [0, 0x12, 1, 1]);
    }

    #[test]
    fn toc_format1_and_2() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        let t = run(&mut s, &u, &[0x43, 0, 1, 0, 0, 0, 0, 0, 12, 0]).unwrap();
        assert_eq!(t, [0, 0x0A, 1, 1, 0, 0x14, 1, 0, 0, 0, 0, 0]);
        let t = run(&mut s, &u, &[0x43, 2, 1, 0, 0, 0, 0, 0, 12, 0]).unwrap();
        assert_eq!(&t[8..12], &[0, 0, 2, 0]);
        // Old-style format field in the control byte.
        let t2 = run(&mut s, &u, &[0x43, 0, 0, 0, 0, 0, 0, 0, 12, 0x40]).unwrap();
        assert_eq!(t2, [0, 0x0A, 1, 1, 0, 0x14, 1, 0, 0, 0, 0, 0]);
        let t = run(&mut s, &u, &[0x43, 2, 2, 0, 0, 0, 0, 0xFF, 0xFF, 0]).unwrap();
        #[rustfmt::skip]
        let want = [
            0x00, 0x2E, 0x01, 0x01,
            0x01, 0x14, 0x00, 0xA0, 0, 0, 0, 0, 0x01, 0x00, 0x00,
            0x01, 0x14, 0x00, 0xA1, 0, 0, 0, 0, 0x01, 0x00, 0x00,
            0x01, 0x14, 0x00, 0xA2, 0, 0, 0, 0, 0x00, 0x0F, 0x19,
            0x01, 0x14, 0x00, 0x01, 0, 0, 0, 0, 0x00, 0x02, 0x00,
        ];
        assert_eq!(t, want);
        assert_eq!(run(&mut s, &u, &[0x43, 0, 3, 0, 0, 0, 0, 0, 12, 0]), Err(Sense::INVALID_FIELD));
    }

    #[test]
    fn msf_conversion() {
        assert_eq!(lba_to_msf(0), [0, 2, 0]);
        assert_eq!(lba_to_msf(16), [0, 2, 16]);
        // 74 minutes of data.
        assert_eq!(lba_to_msf(74 * 60 * 75 - 150), [74, 0, 0]);
        assert_eq!(lba_to_msf(u32::MAX), [255, 59, 74]);
    }

    #[test]
    fn configuration_cd_and_dvd() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        // Windows: header first (8 bytes), then everything.
        let h = run(&mut s, &u, &[0x46, 0, 0, 0, 0, 0, 0, 0, 8, 0]).unwrap();
        let total = u32::from_be_bytes(h[..4].try_into().unwrap()) as usize + 4;
        assert_eq!(&h[6..8], &[0x00, 0x08]);
        let f = run(&mut s, &u, &[0x46, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(f.len(), total);
        #[rustfmt::skip]
        let want: &[u8] = &[
            0, 0, 0, 0x3C, 0, 0, 0x00, 0x08,
            0x00, 0x00, 0x03, 0x04, 0x00, 0x08, 0x01, 0x00,           // profile list: CD-ROM (current)
            0x00, 0x01, 0x0B, 0x08, 0, 0, 0, 8, 0, 0, 0, 0,             // core, USB
            0x00, 0x02, 0x07, 0x04, 0, 0, 0, 0,                         // morphing
            0x00, 0x03, 0x03, 0x04, 0x29, 0, 0, 0,                      // removable medium
            0x00, 0x10, 0x01, 0x08, 0, 0, 0x08, 0x00, 0x00, 0x01, 0, 0, // random readable 2048 x1
            0x00, 0x1E, 0x09, 0x04, 0, 0, 0, 0,                         // CD read (current)
        ];
        assert_eq!(f, want);
        // RT=2: only the profile list (Linux asks for feature 0 like this).
        let f = run(&mut s, &u, &[0x46, 2, 0, 0, 0, 0, 0, 0, 32, 0]).unwrap();
        assert_eq!(f.len(), 8 + 8);
        // RT=1 from 0x10: random readable and CD read.
        let f = run(&mut s, &u, &[0x46, 1, 0, 0x10, 0, 0, 0, 0, 64, 0]).unwrap();
        assert_eq!(f.len(), 8 + 12 + 8);
        assert_eq!(run(&mut s, &u, &[0x46, 3, 0, 0, 0, 0, 0, 0, 64, 0]), Err(Sense::INVALID_FIELD));

        let d = cd((CD_MAX_BLOCKS + 1) * 2048);
        assert!(d.is_dvd() && !u.is_dvd() && !cd(CD_MAX_BLOCKS * 2048).is_dvd());
        let f = run(&mut s, &d, &[0x46, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(&f[6..8], &[0x00, 0x10]);
        assert_eq!(&f[8..20], &[0, 0, 0x03, 0x08, 0x00, 0x10, 0x01, 0, 0x00, 0x08, 0x00, 0]);
        let tail = &f[f.len() - 16..];
        assert_eq!(tail, &[0x00, 0x1E, 0x08, 0x04, 0, 0, 0, 0, 0x00, 0x1F, 0x05, 0x04, 0, 0, 0, 0]);
        // Random readable blocking 16 for DVD.
        let rr = f.windows(4).position(|w| w == [0x00, 0x10, 0x01, 0x08]).unwrap();
        assert_eq!(&f[rr + 8..rr + 10], &[0, 16]);
        // No medium: current profile 0, random readable not current.
        let mut gone = u;
        gone.present = false;
        let f = run(&mut s, &gone, &[0x46, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0]).unwrap();
        assert_eq!(&f[6..8], &[0, 0]);
    }

    #[test]
    fn event_status_and_unit_attention_after_attach() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        s.medium_changed();
        // GESN is not blocked by the unit attention and reports new media
        // once.
        let e = run(&mut s, &u, &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]).unwrap();
        assert_eq!(e, [0, 6, 0x04, 0x10, 0x02, 0x02, 0, 0]);
        let e = run(&mut s, &u, &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]).unwrap();
        assert_eq!(e, [0, 6, 0x04, 0x10, 0x00, 0x02, 0, 0]);
        // Other classes only: no event available.
        assert_eq!(run(&mut s, &u, &[0x4A, 1, 0, 0, 0x02, 0, 0, 0, 8, 0]).unwrap(), [0, 2, 0x80, 0x10]);
        // Asynchronous mode refused.
        assert_eq!(run(&mut s, &u, &[0x4A, 0, 0, 0, 0x10, 0, 0, 0, 8, 0]), Err(Sense::INVALID_FIELD));
        // The unit attention is still there for TEST UNIT READY.
        assert_eq!(run(&mut s, &u, &[0, 0, 0, 0, 0, 0]), Err(Sense::MEDIUM_CHANGED));
        assert_eq!(run(&mut s, &u, &[0, 0, 0, 0, 0, 0]), Ok(Vec::new()));
        // Eject: media removal event.
        assert_eq!(run(&mut s, &u, &[0x1B, 0, 0, 0, 2, 0]), Ok(Vec::new()));
        assert!(s.events.eject);
        let e = run(&mut s, &u, &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]).unwrap();
        assert_eq!(e[4], 0x03);
    }

    #[test]
    fn capabilities_page() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        // Linux sr get_capabilities: MODE SENSE(10), page 0x2A, 128 bytes.
        let m = run(&mut s, &u, &[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 128, 0]).unwrap();
        assert_eq!(m.len(), 30);
        assert_eq!(&m[..8], &[0, 28, 0, 0, 0, 0, 0, 0]);
        #[rustfmt::skip]
        let page = [
            0x2A, 0x14, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00,
            0x1B, 0x80, 0x00, 0x00, 0x00, 0x30, 0x1B, 0x80,
            0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(&m[8..], &page);
        // sr.c reads these bits: no DVD (bit 3 of byte 2), tray, can eject.
        assert_eq!(m[8 + 2] & 0x08, 0);
        assert_eq!(m[8 + 6] >> 5, 1);
        assert_ne!(m[8 + 6] & 0x08, 0);
        let d = cd((CD_MAX_BLOCKS + 10) * 2048);
        let m = run(&mut s, &d, &[0x5A, 0, 0x3F, 0, 0, 0, 0, 0, 128, 0]).unwrap();
        assert_eq!(m[8 + 2] & 0x08, 0x08);
        // Changeable values all zero; saved values not supported; other
        // pages refused; MODE SENSE(6) header.
        let m = run(&mut s, &u, &[0x5A, 0, 0x6A, 0, 0, 0, 0, 0, 128, 0]).unwrap();
        assert!(m[10..].iter().all(|&b| b == 0));
        assert_eq!(
            run(&mut s, &u, &[0x5A, 0, 0xEA, 0, 0, 0, 0, 0, 128, 0]),
            Err(Sense::SAVING_NOT_SUPPORTED)
        );
        assert_eq!(run(&mut s, &u, &[0x5A, 0, 0x01, 0, 0, 0, 0, 0, 128, 0]), Err(Sense::INVALID_FIELD));
        let m = run(&mut s, &u, &[0x1A, 0, 0x2A, 0, 255, 0]).unwrap();
        assert_eq!((m.len(), m[0], m[4]), (26, 25, 0x2A));
        // Locked state shows after PREVENT.
        run(&mut s, &u, &[0x1E, 0, 0, 0, 1, 0]).unwrap();
        let m = run(&mut s, &u, &[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 128, 0]).unwrap();
        assert_eq!(m[8 + 6], 0x2B);
    }

    #[test]
    fn disc_track_header_mechanism_speed() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        let d = run(&mut s, &u, &[0x51, 0, 0, 0, 0, 0, 0, 0, 34, 0]).unwrap();
        #[rustfmt::skip]
        let want = [
            0x00, 0x20, 0x0E, 0x01, 0x01, 0x01, 0x01, 0x00,
            0x00, 0, 0, 0, 0, 0, 0, 0,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(d, want);
        assert_eq!(run(&mut s, &u, &[0x51, 1, 0, 0, 0, 0, 0, 0, 34, 0]), Err(Sense::INVALID_FIELD));
        // cdrom_get_last_written: READ TRACK INFORMATION by track number 1.
        let t = run(&mut s, &u, &[0x52, 1, 0, 0, 0, 1, 0, 0, 36, 0]).unwrap();
        assert_eq!(t.len(), 36);
        assert_eq!(&t[..8], &[0, 34, 1, 1, 0, 0x04, 0x01, 0]);
        assert_eq!(&t[24..32], &[0, 0, 0x03, 0xE8, 0, 0, 0x03, 0xE7]);
        // By LBA inside the track, by session; invalid track numbers.
        assert!(run(&mut s, &u, &[0x52, 0, 0, 0, 0x03, 0xE7, 0, 0, 36, 0]).is_ok());
        assert_eq!(run(&mut s, &u, &[0x52, 0, 0, 0, 0x03, 0xE8, 0, 0, 36, 0]), Err(Sense::LBA_OUT_OF_RANGE));
        assert!(run(&mut s, &u, &[0x52, 2, 0, 0, 0, 1, 0, 0, 36, 0]).is_ok());
        assert_eq!(run(&mut s, &u, &[0x52, 1, 0, 0, 0, 0xFF, 0, 0, 36, 0]), Err(Sense::INVALID_FIELD));
        // READ HEADER in MSF.
        let h = run(&mut s, &u, &[0x44, 2, 0, 0, 0, 16, 0, 0, 8, 0]).unwrap();
        assert_eq!(h, [1, 0, 0, 0, 0, 0, 2, 16]);
        assert_eq!(run(&mut s, &u, &[0x44, 0, 0, 0, 0x03, 0xE8, 0, 0, 8, 0]), Err(Sense::LBA_OUT_OF_RANGE));
        assert_eq!(run(&mut s, &u, &[0xBD, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0]).unwrap(), [0; 8]);
        assert_eq!(run(&mut s, &u, &[0xBB, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0]), Ok(Vec::new()));
    }

    #[test]
    fn dvd_structure() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        let dvd_blocks = CD_MAX_BLOCKS + 0x100;
        let d = cd(dvd_blocks * 2048);
        // Linux dvd_read_physical asks for 4 + 17 bytes; Windows for 2052.
        let p = run(&mut s, &d, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0]).unwrap();
        let end = 0x30000 + dvd_blocks as u32 - 1;
        let mut want = std::vec![0x08, 0x02, 0, 0, 0x01, 0x0F, 0x01, 0x00, 0x00, 0x03, 0x00, 0x00];
        want.extend_from_slice(&end.to_be_bytes());
        want.extend_from_slice(&[0, 0, 0, 0, 0]);
        assert_eq!(p, want);
        let p = run(&mut s, &d, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x04, 0, 0]).unwrap();
        assert_eq!(p.len(), 2052);
        assert!(p[21..].iter().all(|&b| b == 0));
        assert_eq!(
            run(&mut s, &u, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0]),
            Err(Sense::INCOMPATIBLE_FORMAT)
        );
        assert_eq!(run(&mut s, &d, &[0xAD, 0, 0, 0, 0, 0, 0, 1, 0, 21, 0, 0]), Err(Sense::INVALID_FIELD));
    }

    #[test]
    fn reads_and_refusals() {
        let mut s = Scsi::new();
        let u = cd(SMALL + 100); // not a multiple of 2048: one padded block
        assert_eq!(u.blocks, 1001);
        let c = run(&mut s, &u, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(c, [0, 0, 0x03, 0xE8, 0, 0, 0x08, 0x00]);
        assert_eq!(s.command(&u, &[0x28, 0, 0, 0, 0, 16, 0, 0, 2, 0]), Plan::Read { lba: 16, count: 2 });
        assert_eq!(
            s.command(&u, &[0xA8, 0, 0, 0, 0, 16, 0, 0, 0, 2, 0, 0]),
            Plan::Read { lba: 16, count: 2 }
        );
        // READ CD, mode 1, user data only.
        assert_eq!(
            s.command(&u, &[0xBE, 0x08, 0, 0, 0, 16, 0, 0, 2, 0x10, 0, 0]),
            Plan::Read { lba: 16, count: 2 }
        );
        assert_eq!(s.command(&u, &[0xBE, 0x04, 0, 0, 0, 16, 0, 0, 2, 0x10, 0, 0]), Plan::Fail);
        assert_eq!(s.command(&u, &[0xBE, 0, 0, 0, 0, 16, 0, 0, 2, 0xF8, 0, 0]), Plan::Fail);
        // Writes are not CD commands.
        assert_eq!(s.command(&u, &[0x2A, 0, 0, 0, 0, 0, 0, 0, 1, 0]), Plan::Fail);
        assert_eq!(s.sense(), Sense::INVALID_OPCODE);
        // Unknown MMC command, e.g. GET PERFORMANCE.
        assert_eq!(run(&mut s, &u, &[0xAC, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Err(Sense::INVALID_OPCODE));
        // Short CDBs never panic.
        for op in 0..=255u8 {
            let _ = s.command(&u, &[op]);
        }
    }

    #[test]
    fn inquiry_and_no_medium() {
        let mut s = Scsi::new();
        let mut u = cd(SMALL);
        let i = run(&mut s, &u, &[0x12, 0, 0, 0, 36, 0]).unwrap();
        assert_eq!((i[0], i[1]), (0x05, 0x80));
        u.present = false;
        assert_eq!(run(&mut s, &u, &[0x43, 0, 0, 0, 0, 0, 0, 0, 12, 0]), Err(Sense::NOT_PRESENT));
        // Drive queries still work without a disc.
        assert!(run(&mut s, &u, &[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 128, 0]).is_ok());
        let e = run(&mut s, &u, &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]).unwrap();
        assert_eq!(e[5], 0x00);
    }

    /// Linux `sr` probe and first open, in order.
    #[test]
    fn linux_sr_sequence() {
        let mut s = Scsi::new();
        let u = cd(SMALL);
        s.medium_changed();
        let script: &[(&[u8], bool)] = &[
            (&[0x12, 0, 0, 0, 36, 0], true),                              // INQUIRY
            (&[0x00, 0, 0, 0, 0, 0], false),                              // TUR: unit attention
            (&[0x03, 0, 0, 0, 96, 0], true),                              // REQUEST SENSE
            (&[0x00, 0, 0, 0, 0, 0], true),                               // TUR
            (&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], true),                   // READ CAPACITY
            (&[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 128, 0], true),              // capabilities
            (&[0x46, 2, 0, 0, 0, 0, 0, 0, 8, 0], true),                   // GET CONFIGURATION, feature 0
            (&[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0], true),                // GESN poll
            (&[0x51, 0, 0, 0, 0, 0, 0, 0, 2, 0], true),                   // disc info, length
            (&[0x51, 0, 0, 0, 0, 0, 0, 0, 34, 0], true),                  // disc info
            (&[0x52, 1, 0, 0, 0, 1, 0, 0, 36, 0], true),                  // track info, last track
            (&[0x43, 2, 0, 0, 0, 0, 0, 0, 12, 0], true),                  // TOC header (MSF)
            (&[0x43, 2, 0, 0, 0, 0, 1, 0, 12, 0], true),                  // TOC entry track 1
            (&[0x43, 2, 0, 0, 0, 0, 0xAA, 0, 12, 0], true),               // TOC entry lead-out
            (&[0x43, 0, 1, 0, 0, 0, 0, 0, 12, 0], true),                  // multisession
            (&[0xBB, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0], true), // SET CD SPEED max
            (&[0x1E, 0, 0, 0, 1, 0], true),                               // lock door
            (&[0x28, 0, 0, 0, 0, 16, 0, 0, 1, 0], true),                  // read the PVD
            (&[0x1E, 0, 0, 0, 0, 0], true),                               // unlock
            (&[0x1B, 0, 0, 0, 2, 0], true),                               // eject
        ];
        for (i, (cdb, ok)) in script.iter().enumerate() {
            let r = run(&mut s, &u, cdb);
            assert_eq!(r.is_ok(), *ok, "step {i}: {cdb:02X?} -> {r:?}");
        }
        let sense = run(&mut s, &u, &[0x03, 0, 0, 0, 18, 0]).unwrap();
        assert_eq!(sense[2], 0);
    }

    /// Windows `cdrom.sys` start-up, then EDK2 boot (INQUIRY, TUR until
    /// ready, READ CAPACITY, READ(10) of the boot record at sector 17).
    #[test]
    fn windows_and_uefi_sequences() {
        let mut s = Scsi::new();
        let d = cd((CD_MAX_BLOCKS + 1000) * 2048);
        s.medium_changed();
        let windows: &[&[u8]] = &[
            &[0x12, 0, 0, 0, 0x24, 0],
            &[0x46, 0, 0, 0, 0, 0, 0, 0, 8, 0],
            &[0x46, 0, 0, 0, 0, 0, 0, 0x01, 0x00, 0],
            &[0x46, 2, 0, 0x1F, 0, 0, 0, 0, 16, 0],
            &[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 0x2E, 0],
            &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0],
            &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0x43, 2, 0, 0, 0, 0, 0, 0x03, 0x24, 0],
            &[0x43, 2, 1, 0, 0, 0, 0, 0, 12, 0],
            &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x04, 0, 0],
        ];
        // The first media access reports the unit attention, as on a real
        // drive; Windows retries.
        let mut ua = 0;
        for cdb in windows {
            match run(&mut s, &d, cdb) {
                Ok(_) => {}
                Err(Sense::MEDIUM_CHANGED) => {
                    ua += 1;
                    run(&mut s, &d, cdb).unwrap();
                }
                Err(e) => panic!("{cdb:02X?}: {e:?}"),
            }
        }
        assert_eq!(ua, 1);
        let f = run(&mut s, &d, &[0x46, 2, 0, 0x1F, 0, 0, 0, 0, 16, 0]).unwrap();
        assert_eq!(&f[8..12], &[0x00, 0x1F, 0x05, 0x04]);

        let mut s = Scsi::new();
        let u = cd(SMALL);
        s.medium_changed();
        assert!(run(&mut s, &u, &[0x12, 0, 0, 0, 36, 0]).is_ok());
        let mut tries = 0;
        while run(&mut s, &u, &[0, 0, 0, 0, 0, 0]).is_err() {
            run(&mut s, &u, &[0x03, 0, 0, 0, 18, 0]).unwrap();
            tries += 1;
            assert!(tries < 3);
        }
        assert_eq!(run(&mut s, &u, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap()[6..], [0x08, 0]);
        assert_eq!(s.command(&u, &[0x28, 0, 0, 0, 0, 17, 0, 0, 1, 0]), Plan::Read { lba: 17, count: 1 });
        let _ = DeviceKind::CdRom;
    }
}
