//! SCSI command interpretation for a logical unit: disk (SBC subset used
//! by Linux, Windows, macOS and firmware boot menus) or CD/DVD-ROM (the MMC
//! commands in [`super::mmc`], 06-connectivity §3.3).

/// Sense key / ASC / ASCQ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sense {
    pub key: u8,
    pub asc: u8,
    pub ascq: u8,
}

impl Sense {
    pub const NONE: Sense = Sense { key: 0, asc: 0, ascq: 0 };
    pub const INVALID_OPCODE: Sense = Sense { key: 0x05, asc: 0x20, ascq: 0 };
    pub const INVALID_FIELD: Sense = Sense { key: 0x05, asc: 0x24, ascq: 0 };
    pub const LBA_OUT_OF_RANGE: Sense = Sense { key: 0x05, asc: 0x21, ascq: 0 };
    pub const NOT_PRESENT: Sense = Sense { key: 0x02, asc: 0x3A, ascq: 0 };
    pub const WRITE_PROTECTED: Sense = Sense { key: 0x07, asc: 0x27, ascq: 0 };
    pub const MEDIUM_CHANGED: Sense = Sense { key: 0x06, asc: 0x28, ascq: 0 };
    pub const READ_ERROR: Sense = Sense { key: 0x03, asc: 0x11, ascq: 0 };
    pub const WRITE_ERROR: Sense = Sense { key: 0x03, asc: 0x0C, ascq: 0 };
    pub const REMOVAL_PREVENTED: Sense = Sense { key: 0x05, asc: 0x53, ascq: 0x02 };
    pub const SAVING_NOT_SUPPORTED: Sense = Sense { key: 0x05, asc: 0x39, ascq: 0 };
    /// CANNOT READ MEDIUM - INCOMPATIBLE FORMAT (a DVD-only command on a CD).
    pub const INCOMPATIBLE_FORMAT: Sense = Sense { key: 0x05, asc: 0x30, ascq: 0x02 };
}

/// Peripheral device type of the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// Type 0, direct access (card reader, disk image).
    Disk,
    /// Type 5, CD/DVD-ROM with 2048-byte blocks (ISO image).
    CdRom,
}

/// What the unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub kind: DeviceKind,
    pub removable: bool,
    pub block_size: u32,
    pub blocks: u64,
    pub read_only: bool,
    pub present: bool,
    pub vendor: [u8; 8],
    pub product: [u8; 16],
    pub revision: [u8; 4],
}

/// The next step for the transport.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan<'a> {
    /// Command done, no data.
    Done,
    /// Send these bytes to the host.
    DataIn(&'a [u8]),
    /// Read `count` blocks from `lba` and send them.
    Read { lba: u64, count: u32 },
    /// Receive `count` blocks and write them at `lba`.
    Write { lba: u64, count: u32 },
    /// Send `head` followed by zeros up to `len` bytes in total (already
    /// cut to the allocation length). Used for the 2052-byte DVD
    /// structure without a 2 KiB buffer.
    DataInPadded { head: &'a [u8], len: usize },
    /// Check condition (sense recorded).
    Fail,
}

/// Side effects the transport reports upward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    pub eject: bool,
    pub prevent_removal: bool,
    /// Vendor `CB 21`: reboot into the boot ROM's ADFU (compatibility with
    /// the reference tools).
    pub reboot_adfu: bool,
}

pub struct Scsi {
    pub(super) sense: Sense,
    pub(super) unit_attention: bool,
    /// Pending GET EVENT STATUS NOTIFICATION media event code (MMC: 0 none,
    /// 2 new media, 3 media removal).
    pub(super) media_event: u8,
    pub(super) buf: [u8; 128],
    pub events: Events,
    pub commands: u32,
}

impl Default for Scsi {
    fn default() -> Self {
        Self::new()
    }
}

/// Big-endian fields of a CDB; bytes past the end of a short CDB read as 0.
pub(super) fn be32(b: &[u8], o: usize) -> u32 {
    let g = |i: usize| b.get(o + i).copied().unwrap_or(0);
    u32::from_be_bytes([g(0), g(1), g(2), g(3)])
}
pub(super) fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b.get(o).copied().unwrap_or(0), b.get(o + 1).copied().unwrap_or(0)])
}

impl Scsi {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sense: Sense::NONE,
            unit_attention: false,
            media_event: 0,
            buf: [0; 128],
            events: Events { eject: false, prevent_removal: false, reboot_adfu: false },
            commands: 0,
        }
    }

    /// Report "medium may have changed" on the next media access (and a
    /// "new media" event to GET EVENT STATUS NOTIFICATION). Call after
    /// attaching an image.
    pub fn medium_changed(&mut self) {
        self.unit_attention = true;
        self.media_event = 2;
    }

    pub fn set_sense(&mut self, s: Sense) {
        self.sense = s;
    }

    #[must_use]
    pub fn sense(&self) -> Sense {
        self.sense
    }

    pub(super) fn fail(&mut self, s: Sense) -> Plan<'_> {
        self.sense = s;
        Plan::Fail
    }

    pub(super) fn data(&mut self, n: usize, alloc: usize) -> Plan<'_> {
        Plan::DataIn(&self.buf[..n.min(alloc)])
    }

    /// Interpret one CDB.
    pub fn command(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        self.commands = self.commands.wrapping_add(1);
        let Some(&op) = cdb.first() else { return self.fail(Sense::INVALID_OPCODE) };
        // Commands that work without a medium and do not consume a unit
        // attention.
        match op {
            0x03 => return self.request_sense(cdb),
            0x12 => return self.inquiry(u, cdb),
            0xA0 => {
                // REPORT LUNS: one LUN, number 0.
                self.buf[..16].fill(0);
                self.buf[3] = 8;
                let alloc = be32(cdb, 6) as usize;
                return self.data(16, alloc);
            }
            0xCC => {
                // Actions vendor identify (reference tools).
                self.buf[..11].copy_from_slice(b"ACTIONSUSBD");
                return self.data(11, usize::from(cdb.get(7).copied().unwrap_or(11)).max(11));
            }
            0xCB if cdb.get(1) == Some(&0x21) => {
                self.events.reboot_adfu = true;
                self.buf[0] = 0xFF;
                self.buf[1] = 0;
                return self.data(2, 2);
            }
            _ => {}
        }
        let cd = u.kind == DeviceKind::CdRom;
        if cd && super::mmc::medium_independent(op) {
            return self.mmc(u, cdb);
        }
        if !u.present {
            return self.fail(Sense::NOT_PRESENT);
        }
        if self.unit_attention {
            self.unit_attention = false;
            return self.fail(Sense::MEDIUM_CHANGED);
        }
        match op {
            0x00 => Plan::Done, // TEST UNIT READY
            0x1B => {
                // START STOP UNIT: LoEj + !Start = eject.
                if cdb.get(4).copied().unwrap_or(0) & 3 == 2 {
                    if self.events.prevent_removal {
                        return self.fail(Sense::REMOVAL_PREVENTED);
                    }
                    self.events.eject = true;
                    self.media_event = 3;
                }
                Plan::Done
            }
            0x1E => {
                self.events.prevent_removal = cdb.get(4).copied().unwrap_or(0) & 1 != 0;
                Plan::Done
            }
            0x35 | 0x91 => Plan::Done, // SYNCHRONIZE CACHE (10/16): writes are synchronous
            0x2F => Plan::Done,        // VERIFY (10): accept
            0x25 => {
                // READ CAPACITY (10)
                let last = u.blocks.saturating_sub(1).min(0xFFFF_FFFF) as u32;
                self.buf[..4].copy_from_slice(&last.to_be_bytes());
                self.buf[4..8].copy_from_slice(&u.block_size.to_be_bytes());
                self.data(8, 8)
            }
            0x9E if cdb.get(1).copied().unwrap_or(0) & 0x1F == 0x10 => {
                // READ CAPACITY (16)
                self.buf[..32].fill(0);
                self.buf[..8].copy_from_slice(&u.blocks.saturating_sub(1).to_be_bytes());
                self.buf[8..12].copy_from_slice(&u.block_size.to_be_bytes());
                let alloc = be32(cdb, 10) as usize;
                self.data(32, alloc)
            }
            0x23 => {
                // READ FORMAT CAPACITIES
                self.buf[..12].fill(0);
                self.buf[3] = 8;
                self.buf[4..8].copy_from_slice(&(u.blocks.min(0xFFFF_FFFF) as u32).to_be_bytes());
                self.buf[8] = 2; // formatted media
                self.buf[9..12].copy_from_slice(&u.block_size.to_be_bytes()[1..]);
                let alloc = usize::from(be16(cdb, 7));
                self.data(12, alloc)
            }
            0x1A | 0x5A => self.mode_sense(u, cdb),
            0x2A | 0xAA | 0x8A if cd => self.fail(Sense::INVALID_OPCODE),
            0x28 | 0xA8 | 0x88 | 0x2A | 0xAA | 0x8A => self.rw(u, cdb),
            _ if cd => self.mmc(u, cdb),
            _ => self.fail(Sense::INVALID_OPCODE),
        }
    }

    fn rw(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let op = cdb[0];
        let (lba, count) = match op {
            0x28 | 0x2A if cdb.len() >= 10 => (u64::from(be32(cdb, 2)), u32::from(be16(cdb, 7))),
            0xA8 | 0xAA if cdb.len() >= 12 => (u64::from(be32(cdb, 2)), be32(cdb, 6)),
            0x88 | 0x8A if cdb.len() >= 16 => {
                (u64::from(be32(cdb, 2)) << 32 | u64::from(be32(cdb, 6)), be32(cdb, 10))
            }
            _ => return self.fail(Sense::INVALID_FIELD),
        };
        if lba.checked_add(u64::from(count)).is_none_or(|e| e > u.blocks) {
            return self.fail(Sense::LBA_OUT_OF_RANGE);
        }
        let write = matches!(op, 0x2A | 0xAA | 0x8A);
        if write && u.read_only {
            return self.fail(Sense::WRITE_PROTECTED);
        }
        if count == 0 {
            return Plan::Done;
        }
        if write { Plan::Write { lba, count } } else { Plan::Read { lba, count } }
    }

    fn request_sense(&mut self, cdb: &[u8]) -> Plan<'_> {
        let s = self.sense;
        self.sense = Sense::NONE;
        self.buf[..18].fill(0);
        self.buf[0] = 0x70;
        self.buf[2] = s.key;
        self.buf[7] = 10;
        self.buf[12] = s.asc;
        self.buf[13] = s.ascq;
        let alloc = usize::from(cdb.get(4).copied().unwrap_or(18));
        self.data(18, alloc)
    }

    fn inquiry(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let alloc = usize::from(be16(cdb, 3));
        if cdb.get(1).copied().unwrap_or(0) & 1 != 0 {
            // VPD: only page 0 (supported pages) and 0x80 (serial).
            match cdb.get(2).copied().unwrap_or(0) {
                0x00 => {
                    self.buf[..6].copy_from_slice(&[0, 0, 0, 2, 0x00, 0x80]);
                    return self.data(6, alloc);
                }
                0x80 => {
                    self.buf[..8].copy_from_slice(&[0, 0x80, 0, 4, b'0', b'0', b'0', b'1']);
                    return self.data(8, alloc);
                }
                _ => return self.fail(Sense::INVALID_FIELD),
            }
        }
        self.buf[..36].fill(0);
        self.buf[0] = match u.kind {
            DeviceKind::Disk => 0x00,
            DeviceKind::CdRom => 0x05,
        };
        self.buf[1] = if u.removable { 0x80 } else { 0 };
        self.buf[2] = 0x06; // SPC-4
        self.buf[3] = 0x02;
        self.buf[4] = 31;
        self.buf[8..16].copy_from_slice(&u.vendor);
        self.buf[16..32].copy_from_slice(&u.product);
        self.buf[32..36].copy_from_slice(&u.revision);
        self.data(36, alloc)
    }

    fn mode_sense(&mut self, u: &Unit, cdb: &[u8]) -> Plan<'_> {
        let ten = cdb[0] == 0x5A;
        let page = cdb.get(2).copied().unwrap_or(0) & 0x3F;
        let wp = if u.read_only { 0x80 } else { 0 };
        // Header plus, for pages 0x08 (caching) or 0x3F (all), a caching page
        // reporting write cache disabled.
        let (hdr, alloc) = if ten {
            (8usize, usize::from(be16(cdb, 7)))
        } else {
            (4usize, usize::from(cdb.get(4).copied().unwrap_or(0)))
        };
        self.buf[..hdr + 20].fill(0);
        let mut n = hdr;
        if page == 0x08 || page == 0x3F {
            self.buf[n] = 0x08;
            self.buf[n + 1] = 18;
            n += 20;
        } else if page != 0x00 && page != 0x1C {
            return self.fail(Sense::INVALID_FIELD);
        }
        if ten {
            self.buf[..2].copy_from_slice(&((n - 2) as u16).to_be_bytes());
            self.buf[3] = wp;
        } else {
            self.buf[0] = (n - 1) as u8;
            self.buf[2] = wp;
        }
        self.data(n, alloc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Unit {
        Unit {
            kind: DeviceKind::Disk,
            removable: true,
            block_size: 512,
            blocks: 1000,
            read_only: false,
            present: true,
            vendor: *b"orpheus ",
            product: *b"YM03 card       ",
            revision: *b"0001",
        }
    }

    #[test]
    fn inquiry_and_capacity() {
        let mut s = Scsi::new();
        let u = unit();
        match s.command(&u, &[0x12, 0, 0, 0, 36, 0]) {
            Plan::DataIn(d) => {
                assert_eq!(d.len(), 36);
                assert_eq!(d[1], 0x80);
                assert_eq!(&d[8..16], b"orpheus ");
            }
            p => panic!("{p:?}"),
        }
        match s.command(&u, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]) {
            Plan::DataIn(d) => assert_eq!(d, &[0, 0, 3, 0xE7, 0, 0, 2, 0]),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn read_write_ranges() {
        let mut s = Scsi::new();
        let u = unit();
        assert_eq!(s.command(&u, &[0x28, 0, 0, 0, 0, 10, 0, 0, 8, 0]), Plan::Read { lba: 10, count: 8 });
        assert_eq!(s.command(&u, &[0x2A, 0, 0, 0, 3, 0xE0, 0, 0, 8, 0]), Plan::Write { lba: 992, count: 8 });
        assert_eq!(s.command(&u, &[0x28, 0, 0, 0, 3, 0xE1, 0, 0, 8, 0]), Plan::Fail);
        assert_eq!(s.sense(), Sense::LBA_OUT_OF_RANGE);
        let mut ro = unit();
        ro.read_only = true;
        assert_eq!(s.command(&ro, &[0x2A, 0, 0, 0, 0, 0, 0, 0, 1, 0]), Plan::Fail);
        assert_eq!(s.sense(), Sense::WRITE_PROTECTED);
        match s.command(&ro, &[0x1A, 0, 0x3F, 0, 192, 0]) {
            Plan::DataIn(d) => assert_eq!(d[2] & 0x80, 0x80),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn sense_flow_and_attention() {
        let mut s = Scsi::new();
        let u = unit();
        assert_eq!(s.command(&u, &[0xEE, 0, 0, 0, 0, 0]), Plan::Fail);
        match s.command(&u, &[0x03, 0, 0, 0, 18, 0]) {
            Plan::DataIn(d) => assert_eq!((d[2], d[12]), (0x05, 0x20)),
            p => panic!("{p:?}"),
        }
        s.medium_changed();
        assert_eq!(s.command(&u, &[0x00, 0, 0, 0, 0, 0]), Plan::Fail);
        assert_eq!(s.sense(), Sense::MEDIUM_CHANGED);
        assert_eq!(s.command(&u, &[0x00, 0, 0, 0, 0, 0]), Plan::Done);
        let mut gone = unit();
        gone.present = false;
        assert_eq!(s.command(&gone, &[0x00, 0, 0, 0, 0, 0]), Plan::Fail);
        assert_eq!(s.sense(), Sense::NOT_PRESENT);
    }

    #[test]
    fn eject_and_prevent() {
        let mut s = Scsi::new();
        let u = unit();
        assert_eq!(s.command(&u, &[0x1E, 0, 0, 0, 1, 0]), Plan::Done);
        assert_eq!(s.command(&u, &[0x1B, 0, 0, 0, 2, 0]), Plan::Fail);
        assert_eq!(s.command(&u, &[0x1E, 0, 0, 0, 0, 0]), Plan::Done);
        assert_eq!(s.command(&u, &[0x1B, 0, 0, 0, 2, 0]), Plan::Done);
        assert!(s.events.eject);
    }

    #[test]
    fn vendor_compat() {
        let mut s = Scsi::new();
        let u = unit();
        match s.command(&u, &[0xCC, 0, 0, 0, 0, 0, 0, 11]) {
            Plan::DataIn(d) => assert_eq!(d, b"ACTIONSUSBD"),
            p => panic!("{p:?}"),
        }
        let _ = s.command(&u, &[0xCB, 0x21, 0, 0, 0, 0, 0, 2]);
        assert!(s.events.reboot_adfu);
    }

    #[test]
    fn read16_and_capacity16() {
        let mut s = Scsi::new();
        let mut u = unit();
        u.blocks = 0x1_0000_0000 + 5;
        let mut c = [0u8; 16];
        c[0] = 0x88;
        c[2..10].copy_from_slice(&0x1_0000_0000u64.to_be_bytes());
        c[10..14].copy_from_slice(&5u32.to_be_bytes());
        assert_eq!(s.command(&u, &c), Plan::Read { lba: 0x1_0000_0000, count: 5 });
        let mut c = [0u8; 16];
        c[0] = 0x9E;
        c[1] = 0x10;
        c[13] = 32;
        match s.command(&u, &c) {
            Plan::DataIn(d) => assert_eq!(u64::from_be_bytes(d[..8].try_into().unwrap()), 0x1_0000_0004),
            p => panic!("{p:?}"),
        }
    }
}
