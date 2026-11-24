//! Fault record: `0x137D00..0x137E00`.
//!
//! Written by the firmware's fault handlers (resident code) before the crash
//! screen and the watchdog reboot; read by the host through ADFU.

use crate::{magic, rd32, wr32};

pub const BASE: u32 = 0x0013_7D00;
pub const SIZE: usize = 0x100;
pub const MAGIC: u32 = magic(b"FALT");

pub const OFF_MAGIC: usize = 0x00;
pub const OFF_KIND: usize = 0x04;
pub const OFF_PC: usize = 0x08;
pub const OFF_LR: usize = 0x0C;
pub const OFF_XPSR: usize = 0x10;
pub const OFF_SP: usize = 0x14;
pub const OFF_CFSR: usize = 0x18;
pub const OFF_HFSR: usize = 0x1C;
pub const OFF_MMFAR: usize = 0x20;
pub const OFF_BFAR: usize = 0x24;
/// IPSR at the time of the fault (0 = thread mode).
pub const OFF_IPSR: usize = 0x28;
pub const OFF_EXC_RETURN: usize = 0x2C;
/// Pager state words (fault address being served, page, line) or 0.
pub const OFF_PAGER: usize = 0x30;
/// Stacked r0..r3, r12.
pub const OFF_REGS: usize = 0x40;
/// How many faults were recorded since the counter was last cleared.
pub const OFF_COUNT: usize = 0x54;
/// Build id (16 bytes) of the firmware that faulted.
pub const OFF_BUILD_ID: usize = 0x60;
/// Panic message or extra text: NUL-padded ASCII.
pub const OFF_TEXT: usize = 0x80;
pub const TEXT_LEN: usize = 0x80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    HardFault = 3,
    MemManage = 4,
    BusFault = 5,
    UsageFault = 6,
    /// MPU stack guard hit.
    StackGuard = 0x100,
    /// Page fault while an exception was active, or inside the pager.
    PageFaultInException = 0x101,
    Panic = 0x102,
    /// Watchdog bite observed at boot (no other record).
    Watchdog = 0x103,
    /// Unexpected interrupt (no handler).
    UnhandledIrq = 0x104,
}

impl Kind {
    #[must_use]
    pub fn from_word(w: u32) -> Option<Self> {
        [
            Self::HardFault,
            Self::MemManage,
            Self::BusFault,
            Self::UsageFault,
            Self::StackGuard,
            Self::PageFaultInException,
            Self::Panic,
            Self::Watchdog,
            Self::UnhandledIrq,
        ]
        .into_iter()
        .find(|k| *k as u32 == w)
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::HardFault => "HardFault",
            Self::MemManage => "MemManage",
            Self::BusFault => "BusFault",
            Self::UsageFault => "UsageFault",
            Self::StackGuard => "StackGuard",
            Self::PageFaultInException => "PageFaultInException",
            Self::Panic => "Panic",
            Self::Watchdog => "Watchdog",
            Self::UnhandledIrq => "UnhandledIrq",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultRecord {
    pub kind: u32,
    pub pc: u32,
    pub lr: u32,
    pub xpsr: u32,
    pub sp: u32,
    pub cfsr: u32,
    pub hfsr: u32,
    pub mmfar: u32,
    pub bfar: u32,
    pub ipsr: u32,
    pub exc_return: u32,
    pub pager: [u32; 4],
    pub regs: [u32; 5],
    pub count: u32,
    pub build_id: [u8; 16],
    pub text: [u8; TEXT_LEN],
}

impl Default for FaultRecord {
    fn default() -> Self {
        Self {
            kind: 0,
            pc: 0,
            lr: 0,
            xpsr: 0,
            sp: 0,
            cfsr: 0,
            hfsr: 0,
            mmfar: 0,
            bfar: 0,
            ipsr: 0,
            exc_return: 0,
            pager: [0; 4],
            regs: [0; 5],
            count: 0,
            build_id: [0; 16],
            text: [0; TEXT_LEN],
        }
    }
}

impl FaultRecord {
    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if rd32(b, OFF_MAGIC)? != MAGIC {
            return None;
        }
        let mut pager = [0u32; 4];
        for (i, p) in pager.iter_mut().enumerate() {
            *p = rd32(b, OFF_PAGER + 4 * i)?;
        }
        let mut regs = [0u32; 5];
        for (i, r) in regs.iter_mut().enumerate() {
            *r = rd32(b, OFF_REGS + 4 * i)?;
        }
        let mut build_id = [0u8; 16];
        build_id.copy_from_slice(b.get(OFF_BUILD_ID..OFF_BUILD_ID + 16)?);
        let mut text = [0u8; TEXT_LEN];
        text.copy_from_slice(b.get(OFF_TEXT..OFF_TEXT + TEXT_LEN)?);
        Some(Self {
            kind: rd32(b, OFF_KIND)?,
            pc: rd32(b, OFF_PC)?,
            lr: rd32(b, OFF_LR)?,
            xpsr: rd32(b, OFF_XPSR)?,
            sp: rd32(b, OFF_SP)?,
            cfsr: rd32(b, OFF_CFSR)?,
            hfsr: rd32(b, OFF_HFSR)?,
            mmfar: rd32(b, OFF_MMFAR)?,
            bfar: rd32(b, OFF_BFAR)?,
            ipsr: rd32(b, OFF_IPSR)?,
            exc_return: rd32(b, OFF_EXC_RETURN)?,
            pager,
            regs,
            count: rd32(b, OFF_COUNT)?,
            build_id,
            text,
        })
    }

    #[must_use]
    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        wr32(&mut b, OFF_MAGIC, MAGIC);
        wr32(&mut b, OFF_KIND, self.kind);
        wr32(&mut b, OFF_PC, self.pc);
        wr32(&mut b, OFF_LR, self.lr);
        wr32(&mut b, OFF_XPSR, self.xpsr);
        wr32(&mut b, OFF_SP, self.sp);
        wr32(&mut b, OFF_CFSR, self.cfsr);
        wr32(&mut b, OFF_HFSR, self.hfsr);
        wr32(&mut b, OFF_MMFAR, self.mmfar);
        wr32(&mut b, OFF_BFAR, self.bfar);
        wr32(&mut b, OFF_IPSR, self.ipsr);
        wr32(&mut b, OFF_EXC_RETURN, self.exc_return);
        for (i, p) in self.pager.iter().enumerate() {
            wr32(&mut b, OFF_PAGER + 4 * i, *p);
        }
        for (i, r) in self.regs.iter().enumerate() {
            wr32(&mut b, OFF_REGS + 4 * i, *r);
        }
        wr32(&mut b, OFF_COUNT, self.count);
        b[OFF_BUILD_ID..OFF_BUILD_ID + 16].copy_from_slice(&self.build_id);
        b[OFF_TEXT..OFF_TEXT + TEXT_LEN].copy_from_slice(&self.text);
        b
    }

    #[must_use]
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_word(self.kind)
    }

    #[must_use]
    pub fn text_str(&self) -> &str {
        let n = self.text.iter().position(|&c| c == 0).unwrap_or(TEXT_LEN);
        core::str::from_utf8(&self.text[..n]).unwrap_or("<non-utf8>")
    }
}

/// Decode the CFSR into the names of the set bits (ARMv7-M ARM B3.2.15).
pub fn cfsr_bits(cfsr: u32) -> impl Iterator<Item = &'static str> {
    const NAMES: [(u32, &str); 18] = [
        (0, "IACCVIOL"),
        (1, "DACCVIOL"),
        (3, "MUNSTKERR"),
        (4, "MSTKERR"),
        (5, "MLSPERR"),
        (7, "MMARVALID"),
        (8, "IBUSERR"),
        (9, "PRECISERR"),
        (10, "IMPRECISERR"),
        (11, "UNSTKERR"),
        (12, "STKERR"),
        (13, "LSPERR"),
        (15, "BFARVALID"),
        (16, "UNDEFINSTR"),
        (17, "INVSTATE"),
        (18, "INVPC"),
        (19, "NOCP"),
        (24, "UNALIGNED"),
    ];
    NAMES
        .into_iter()
        .filter(move |(bit, _)| cfsr & (1 << bit) != 0)
        .map(|(_, n)| n)
        .chain((cfsr & (1 << 25) != 0).then_some("DIVBYZERO"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn layout() {
        const { assert!(OFF_TEXT + TEXT_LEN == SIZE) };
        assert_eq!(BASE as usize + SIZE, crate::result::BASE as usize);
    }

    #[test]
    fn roundtrip() {
        let mut r =
            FaultRecord { kind: Kind::BusFault as u32, pc: 0x10_0123, cfsr: 0x8200, ..Default::default() };
        r.text[..4].copy_from_slice(b"boom");
        let back = FaultRecord::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.kind(), Some(Kind::BusFault));
        assert_eq!(back.text_str(), "boom");
    }

    #[test]
    fn cfsr_decode() {
        let v: Vec<_> = cfsr_bits(0x8200 | 1 << 25).collect();
        assert_eq!(v, ["PRECISERR", "BFARVALID", "DIVBYZERO"]);
    }
}
