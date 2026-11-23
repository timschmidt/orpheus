//! Boot mailbox: `0x137FC0..0x138000` (docs/plan/07-tooling-testing.md §5.2).
//!
//! The last three words keep the layout of the C boot stub that is installed
//! today (`yoton/payloads/ym03stub.c`), so either stage 2 understands either
//! firmware:
//!
//! | Offset | Word | Meaning |
//! |---|---|---|
//! | `+0x00` | `request` | [`Request`] the next boot should honour (`0` = none) |
//! | `+0x04` | `test_id` | FNV-1a hash of the HIL test name ([`crate::hash`]) |
//! | `+0x08` | `flags` | [`FLAG_RETURN_ADFU`] … |
//! | `+0x0C..+0x30` | `args[9]` | test arguments |
//! | `+0x30` | `test_mode` | `"YTST"` arms test mode (legacy word, `0x137FF0`) |
//! | `+0x34` | `adfu` | `"ADFU"`: stage 2 enters USB recovery (legacy, `0x137FF4`) |
//! | `+0x38` | `boot_count` | `"BMY" << 8 \| attempts` (legacy, `0x137FF8`) |
//! | `+0x3C` | `boot_info` | written by stage 2: what it decided ([`BootInfo`]) |

use crate::{magic, rd32, wr32};

pub const BASE: u32 = 0x0013_7FC0;
pub const SIZE: usize = 0x40;

pub const OFF_REQUEST: usize = 0x00;
pub const OFF_TEST_ID: usize = 0x04;
pub const OFF_FLAGS: usize = 0x08;
pub const OFF_ARGS: usize = 0x0C;
pub const N_ARGS: usize = 9;
pub const OFF_TEST_MODE: usize = 0x30;
pub const OFF_ADFU: usize = 0x34;
pub const OFF_BOOT_COUNT: usize = 0x38;
pub const OFF_BOOT_INFO: usize = 0x3C;

/// Absolute addresses of the legacy words.
pub const ADDR_TEST_MODE: u32 = BASE + OFF_TEST_MODE as u32;
pub const ADDR_ADFU: u32 = BASE + OFF_ADFU as u32;
pub const ADDR_BOOT_COUNT: u32 = BASE + OFF_BOOT_COUNT as u32;

pub const TEST_MODE_MAGIC: u32 = magic(b"YTST");
pub const ADFU_MAGIC: u32 = magic(b"ADFU");
/// Upper 24 bits of the boot counter word.
pub const BOOT_COUNT_TAG: u32 = 0x0059_4D42;
/// Failed starts in a row before stage 2 falls back to USB recovery.
pub const BOOT_COUNT_LIMIT: u32 = 3;

/// After the test, reboot into ADFU (so the host can read results) instead of
/// the installed firmware.
pub const FLAG_RETURN_ADFU: u32 = 1 << 0;
/// Leave the screen alone (no test banner) — for golden comparisons.
pub const FLAG_QUIET: u32 = 1 << 1;

/// What the next boot should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Request {
    None = 0,
    /// Boot ROM USB recovery.
    Adfu = magic(b"ADFU"),
    /// Stock (vendor) firmware.
    Stock = magic(b"STOK"),
    /// Run the HIL test named by `test_id`.
    Test = magic(b"TEST"),
}

impl Request {
    #[must_use]
    pub fn from_word(w: u32) -> Self {
        match w {
            x if x == Self::Adfu as u32 => Self::Adfu,
            x if x == Self::Stock as u32 => Self::Stock,
            x if x == Self::Test as u32 => Self::Test,
            _ => Self::None,
        }
    }
}

/// Stage 2's record of the boot decision (`boot_info` word):
/// `magic "B" (0x42) << 24 | path << 16 | attempt << 8 | keys`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootInfo {
    pub path: u8,
    pub attempt: u8,
    pub keys: u8,
}

pub const PATH_FIRMWARE: u8 = 1;
pub const PATH_VENDOR: u8 = 2;
pub const PATH_ADFU_KEY: u8 = 3;
pub const PATH_ADFU_MAILBOX: u8 = 4;
pub const PATH_ADFU_COUNTER: u8 = 5;
pub const PATH_ADFU_NOIMAGE: u8 = 6;

impl BootInfo {
    #[must_use]
    pub const fn to_word(self) -> u32 {
        0x42 << 24 | (self.path as u32) << 16 | (self.attempt as u32) << 8 | self.keys as u32
    }
    #[must_use]
    pub const fn from_word(w: u32) -> Option<Self> {
        if w >> 24 != 0x42 {
            return None;
        }
        Some(Self { path: (w >> 16) as u8, attempt: (w >> 8) as u8, keys: w as u8 })
    }
}

/// Host-side image of the mailbox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mailbox {
    pub request: u32,
    pub test_id: u32,
    pub flags: u32,
    pub args: [u32; N_ARGS],
    pub test_mode: u32,
    pub adfu: u32,
    pub boot_count: u32,
    pub boot_info: u32,
}

impl Mailbox {
    /// Arm a HIL test.
    #[must_use]
    pub fn test(test_id: u32, flags: u32, args: &[u32]) -> Self {
        let mut m = Self {
            request: Request::Test as u32,
            test_id,
            flags,
            test_mode: TEST_MODE_MAGIC,
            ..Self::default()
        };
        for (d, s) in m.args.iter_mut().zip(args) {
            *d = *s;
        }
        m
    }

    #[must_use]
    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        wr32(&mut b, OFF_REQUEST, self.request);
        wr32(&mut b, OFF_TEST_ID, self.test_id);
        wr32(&mut b, OFF_FLAGS, self.flags);
        for (i, a) in self.args.iter().enumerate() {
            wr32(&mut b, OFF_ARGS + 4 * i, *a);
        }
        wr32(&mut b, OFF_TEST_MODE, self.test_mode);
        wr32(&mut b, OFF_ADFU, self.adfu);
        wr32(&mut b, OFF_BOOT_COUNT, self.boot_count);
        wr32(&mut b, OFF_BOOT_INFO, self.boot_info);
        b
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut args = [0u32; N_ARGS];
        for (i, a) in args.iter_mut().enumerate() {
            *a = rd32(b, OFF_ARGS + 4 * i)?;
        }
        Some(Self {
            request: rd32(b, OFF_REQUEST)?,
            test_id: rd32(b, OFF_TEST_ID)?,
            flags: rd32(b, OFF_FLAGS)?,
            args,
            test_mode: rd32(b, OFF_TEST_MODE)?,
            adfu: rd32(b, OFF_ADFU)?,
            boot_count: rd32(b, OFF_BOOT_COUNT)?,
            boot_info: rd32(b, OFF_BOOT_INFO)?,
        })
    }

    /// Attempts recorded in the boot counter word (0 if the tag is absent).
    #[must_use]
    pub fn boot_attempts(&self) -> u32 {
        if self.boot_count >> 8 == BOOT_COUNT_TAG { self.boot_count & 0xFF } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_word_addresses() {
        assert_eq!(ADDR_TEST_MODE, 0x137FF0);
        assert_eq!(ADDR_ADFU, 0x137FF4);
        assert_eq!(ADDR_BOOT_COUNT, 0x137FF8);
        assert_eq!(BASE as usize + SIZE, 0x138000);
    }

    #[test]
    fn roundtrip() {
        let m = Mailbox::test(0xdead_beef, FLAG_RETURN_ADFU, &[1, 2, 3]);
        let b = m.to_bytes();
        assert_eq!(Mailbox::from_bytes(&b), Some(m));
        assert_eq!(Request::from_word(m.request), Request::Test);
        assert_eq!(rd32(&b, OFF_TEST_MODE), Some(0x5453_5459));
    }

    #[test]
    fn boot_info_roundtrip() {
        let i = BootInfo { path: PATH_FIRMWARE, attempt: 2, keys: 0x10 };
        assert_eq!(BootInfo::from_word(i.to_word()), Some(i));
        assert_eq!(BootInfo::from_word(0), None);
    }

    #[test]
    fn attempts() {
        let m = Mailbox { boot_count: BOOT_COUNT_TAG << 8 | 2, ..Mailbox::default() };
        assert_eq!(m.boot_attempts(), 2);
        let m = Mailbox { boot_count: 0x1234_5602, ..Mailbox::default() };
        assert_eq!(m.boot_attempts(), 0);
    }
}
