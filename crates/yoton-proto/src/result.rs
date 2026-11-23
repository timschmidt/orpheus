//! HIL result block: `0x137E00..0x137FC0`.
//!
//! Written by a test running on the device, read by the host after the device
//! has rebooted into ADFU (SRAM0 survives the reset).
//!
//! | Offset | Content |
//! |---|---|
//! | `+0x000` | magic `"YRES"` |
//! | `+0x004` | test id (FNV-1a of the name) |
//! | `+0x008` | verdict ([`Verdict`]) |
//! | `+0x00C` | number of counters used (≤ 64) |
//! | `+0x010` | `counters[64]` |
//! | `+0x110` | message: 128 bytes, NUL-padded ASCII |
//! | `+0x190` | sequence number, incremented by every write of a verdict |
//! | `+0x194..` | reserved |

use crate::{magic, rd32, wr32};

pub const BASE: u32 = 0x0013_7E00;
pub const SIZE: usize = 0x1C0;
pub const MAGIC: u32 = magic(b"YRES");

pub const OFF_MAGIC: usize = 0x000;
pub const OFF_TEST_ID: usize = 0x004;
pub const OFF_VERDICT: usize = 0x008;
pub const OFF_NCOUNTERS: usize = 0x00C;
pub const OFF_COUNTERS: usize = 0x010;
pub const N_COUNTERS: usize = 64;
pub const OFF_MESSAGE: usize = 0x110;
pub const MESSAGE_LEN: usize = 128;
pub const OFF_SEQ: usize = 0x190;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Verdict {
    /// The test started but has not finished (a reboot in this state means
    /// it crashed or hung).
    Running = magic(b"RUN."),
    Pass = magic(b"PASS"),
    Fail = magic(b"FAIL"),
    /// Could not run here (missing hardware, precondition).
    Skip = magic(b"SKIP"),
}

impl Verdict {
    #[must_use]
    pub fn from_word(w: u32) -> Option<Self> {
        [Self::Running, Self::Pass, Self::Fail, Self::Skip].into_iter().find(|v| *v as u32 == w)
    }
}

/// Host-side image of the result block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultBlock {
    pub test_id: u32,
    pub verdict: Option<Verdict>,
    pub counters: [u32; N_COUNTERS],
    pub n_counters: usize,
    pub message: [u8; MESSAGE_LEN],
    pub seq: u32,
}

impl ResultBlock {
    /// Parse; `None` if the magic is absent (no test wrote here).
    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if rd32(b, OFF_MAGIC)? != MAGIC {
            return None;
        }
        let mut counters = [0u32; N_COUNTERS];
        for (i, c) in counters.iter_mut().enumerate() {
            *c = rd32(b, OFF_COUNTERS + 4 * i)?;
        }
        let mut message = [0u8; MESSAGE_LEN];
        message.copy_from_slice(b.get(OFF_MESSAGE..OFF_MESSAGE + MESSAGE_LEN)?);
        Some(Self {
            test_id: rd32(b, OFF_TEST_ID)?,
            verdict: Verdict::from_word(rd32(b, OFF_VERDICT)?),
            n_counters: (rd32(b, OFF_NCOUNTERS)? as usize).min(N_COUNTERS),
            counters,
            message,
            seq: rd32(b, OFF_SEQ)?,
        })
    }

    #[must_use]
    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        wr32(&mut b, OFF_MAGIC, MAGIC);
        wr32(&mut b, OFF_TEST_ID, self.test_id);
        wr32(&mut b, OFF_VERDICT, self.verdict.map_or(0, |v| v as u32));
        wr32(&mut b, OFF_NCOUNTERS, self.n_counters as u32);
        for (i, c) in self.counters.iter().enumerate() {
            wr32(&mut b, OFF_COUNTERS + 4 * i, *c);
        }
        b[OFF_MESSAGE..OFF_MESSAGE + MESSAGE_LEN].copy_from_slice(&self.message);
        wr32(&mut b, OFF_SEQ, self.seq);
        b
    }

    /// The message up to the first NUL, lossy for non-ASCII.
    #[must_use]
    pub fn message_str(&self) -> &str {
        let n = self.message.iter().position(|&c| c == 0).unwrap_or(MESSAGE_LEN);
        core::str::from_utf8(&self.message[..n]).unwrap_or("<non-utf8 message>")
    }

    #[must_use]
    pub fn counters(&self) -> &[u32] {
        &self.counters[..self.n_counters]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits() {
        const { assert!(OFF_SEQ + 4 <= SIZE) };
        assert_eq!(BASE as usize + SIZE, crate::mailbox::BASE as usize);
    }

    #[test]
    fn roundtrip() {
        let mut message = [0u8; MESSAGE_LEN];
        message[..5].copy_from_slice(b"hello");
        let mut counters = [0u32; N_COUNTERS];
        counters[0] = 7;
        counters[1] = 9;
        let r = ResultBlock {
            test_id: 5,
            verdict: Some(Verdict::Pass),
            counters,
            n_counters: 2,
            message,
            seq: 1,
        };
        let back = ResultBlock::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.message_str(), "hello");
        assert_eq!(back.counters(), &[7, 9]);
    }

    #[test]
    fn absent() {
        assert!(ResultBlock::from_bytes(&[0u8; SIZE]).is_none());
        assert!(ResultBlock::from_bytes(&[0u8; 3]).is_none());
    }
}
