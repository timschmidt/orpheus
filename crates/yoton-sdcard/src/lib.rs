//! SD protocol pieces that need no hardware (SD Physical Layer Simplified
//! Specification): command numbers and arguments, OCR, R1 status, CSD
//! (versions 1–3) and CID parsing. The controller driver lives in
//! `atj2157-hal::sd`.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod cmd {
    pub const GO_IDLE: u8 = 0;
    pub const ALL_SEND_CID: u8 = 2;
    pub const SEND_RELATIVE_ADDR: u8 = 3;
    pub const SWITCH_FUNC: u8 = 6;
    pub const SELECT_CARD: u8 = 7;
    pub const SEND_IF_COND: u8 = 8;
    pub const SEND_CSD: u8 = 9;
    pub const STOP_TRANSMISSION: u8 = 12;
    pub const SEND_STATUS: u8 = 13;
    pub const SET_BLOCKLEN: u8 = 16;
    pub const READ_SINGLE: u8 = 17;
    pub const READ_MULTIPLE: u8 = 18;
    pub const SET_BLOCK_COUNT: u8 = 23;
    pub const WRITE_SINGLE: u8 = 24;
    pub const WRITE_MULTIPLE: u8 = 25;
    pub const APP_CMD: u8 = 55;
    /// Application commands (after CMD55).
    pub const ACMD_SET_BUS_WIDTH: u8 = 6;
    pub const ACMD_SD_STATUS: u8 = 13;
    pub const ACMD_SEND_OP_COND: u8 = 41;
    pub const ACMD_SEND_SCR: u8 = 51;
}

/// CMD8 argument: 2.7–3.6 V, check pattern 0xAA.
pub const IF_COND_ARG: u32 = 0x1AA;
/// ACMD41 argument: HCS (high capacity supported) + 3.2–3.4 V window.
pub const OP_COND_ARG: u32 = (1 << 30) | 0x0030_0000;

/// OCR (R3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ocr(pub u32);

impl Ocr {
    /// Power-up finished.
    #[must_use]
    pub const fn ready(self) -> bool {
        self.0 & (1 << 31) != 0
    }
    /// Card Capacity Status: block addressed (SDHC/SDXC).
    #[must_use]
    pub const fn high_capacity(self) -> bool {
        self.0 & (1 << 30) != 0
    }
}

/// Card status (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status(pub u32);

/// Current state field of R1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Ready,
    Ident,
    Stby,
    Tran,
    Data,
    Rcv,
    Prg,
    Dis,
    Other(u8),
}

impl Status {
    /// Error bits (out of range, address error, block len, erase, WP,
    /// lock, CRC, illegal command, ECC, CC, generic).
    pub const ERRORS: u32 = 0xFDF9_8008;

    #[must_use]
    pub const fn state(self) -> State {
        match (self.0 >> 9) & 0xF {
            0 => State::Idle,
            1 => State::Ready,
            2 => State::Ident,
            3 => State::Stby,
            4 => State::Tran,
            5 => State::Data,
            6 => State::Rcv,
            7 => State::Prg,
            8 => State::Dis,
            x => State::Other(x as u8),
        }
    }
    #[must_use]
    pub const fn ready_for_data(self) -> bool {
        self.0 & (1 << 8) != 0
    }
    #[must_use]
    pub const fn has_error(self) -> bool {
        self.0 & Self::ERRORS != 0
    }
}

/// A 128-bit register as four words, most significant first (`w[0]` holds
/// bits 127..96), CRC byte included at the bottom as the card sends it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R128(pub [u32; 4]);

impl R128 {
    /// Bits `[hi:lo]` (inclusive) of the 128-bit value.
    #[must_use]
    pub fn bits(&self, hi: u32, lo: u32) -> u32 {
        let mut v: u64 = 0;
        for b in (lo..=hi).rev() {
            let word = 3 - (b / 32) as usize;
            let bit = (self.0[word] >> (b % 32)) & 1;
            v = (v << 1) | u64::from(bit);
        }
        v as u32
    }
}

/// The parts of the CSD the firmware uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Csd {
    pub version: u8,
    /// Capacity in 512-byte blocks.
    pub blocks: u64,
    /// Max transfer rate code (TRAN_SPEED), e.g. 0x32 = 25 MHz, 0x5A = 50 MHz.
    pub tran_speed: u8,
    pub ccc: u16,
}

impl Csd {
    #[must_use]
    pub fn parse(r: &R128) -> Option<Self> {
        let structure = r.bits(127, 126);
        let tran_speed = r.bits(103, 96) as u8;
        let ccc = r.bits(95, 84) as u16;
        let blocks = match structure {
            0 => {
                let read_bl_len = r.bits(83, 80);
                let c_size = u64::from(r.bits(73, 62));
                let mult = r.bits(49, 47);
                let bytes = (c_size + 1) << (mult + 2) << read_bl_len;
                bytes / 512
            }
            1 => (u64::from(r.bits(69, 48)) + 1) * 1024,
            2 => (u64::from(r.bits(75, 48)) + 1) * 1024,
            _ => return None,
        };
        Some(Self { version: structure as u8 + 1, blocks, tran_speed, ccc })
    }

    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.blocks * 512
    }
}

/// Card identification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cid {
    pub manufacturer: u8,
    pub oem: [u8; 2],
    pub name: [u8; 5],
    pub revision: u8,
    pub serial: u32,
    /// (year, month)
    pub date: (u16, u8),
}

impl Cid {
    #[must_use]
    pub fn parse(r: &R128) -> Self {
        let b = |hi, lo| r.bits(hi, lo) as u8;
        Self {
            manufacturer: b(127, 120),
            oem: [b(119, 112), b(111, 104)],
            name: [b(103, 96), b(95, 88), b(87, 80), b(79, 72), b(71, 64)],
            revision: b(63, 56),
            serial: r.bits(55, 24),
            date: (2000 + r.bits(19, 12) as u16, r.bits(11, 8) as u8),
        }
    }
}

/// Bytes ↔ words for a 128-bit register as the card sends it (MSB first).
#[must_use]
pub fn r128_from_bytes(b: &[u8; 16]) -> R128 {
    let mut w = [0u32; 4];
    for (i, x) in w.iter_mut().enumerate() {
        *x = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
    }
    R128(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex16(s: &str) -> [u8; 16] {
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        b
    }

    #[test]
    fn csd_v2_sdhc_32gb() {
        // CSD of a 32 GB SDHC card (C_SIZE = 0xEDC7 → 61 064 MiB).
        let r = r128_from_bytes(&hex16("400E00325B590000EDC77F800A400000"));
        let c = Csd::parse(&r).unwrap();
        assert_eq!(c.version, 2);
        assert_eq!(c.blocks, (0xEDC7 + 1) * 1024);
        assert_eq!(c.tran_speed, 0x32);
    }

    #[test]
    fn csd_v1_1gb() {
        // READ_BL_LEN 9 (512), C_SIZE 0xF13 (bits 73:62), C_SIZE_MULT 7.
        let mut r = R128::default();
        let set = |r: &mut R128, hi: u32, lo: u32, v: u32| {
            for (i, b) in (lo..=hi).enumerate() {
                let word = 3 - (b / 32) as usize;
                if v >> i & 1 != 0 {
                    r.0[word] |= 1 << (b % 32);
                }
            }
        };
        set(&mut r, 83, 80, 9);
        set(&mut r, 73, 62, 0xF13);
        set(&mut r, 49, 47, 7);
        let c = Csd::parse(&r).unwrap();
        assert_eq!(c.version, 1);
        assert_eq!(c.blocks, (0xF13 + 1) << 9);
    }

    #[test]
    fn csd_v3_sduc_rejects_bad_structure() {
        let mut r = R128::default();
        r.0[0] = 3 << 30;
        assert!(Csd::parse(&r).is_none());
    }

    #[test]
    fn cid_fields() {
        // SanDisk-style CID: MID 0x03, OID "SD", PNM "SN64G", PRV 0x80.
        let r = r128_from_bytes(&hex16("035344534E3634478012345678015300"));
        let c = Cid::parse(&r);
        assert_eq!(c.manufacturer, 3);
        assert_eq!(&c.oem, b"SD");
        assert_eq!(&c.name, b"SN64G");
        assert_eq!(c.serial, 0x1234_5678);
        assert_eq!(c.date, (2021, 3));
    }

    #[test]
    fn status_and_ocr() {
        let s = Status(4 << 9 | 1 << 8);
        assert_eq!(s.state(), State::Tran);
        assert!(s.ready_for_data());
        assert!(!s.has_error());
        assert!(Status(1 << 31).has_error());
        let o = Ocr(0xC0FF_8000);
        assert!(o.ready() && o.high_capacity());
    }
}
