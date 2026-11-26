//! A fake HID host: what a computer with a given layout would type from
//! the reports it receives. A key produces its character when it goes down
//! (no auto-repeat); modifiers alone, shortcuts (Ctrl, Alt, GUI) and keys
//! without a character produce nothing.

use crate::layout::{self, Layout};
use crate::report::KeyboardReport;

/// Text kept (the last bytes win when it overflows).
pub const TEXT_MAX: usize = 256;

#[derive(Clone, Debug)]
pub struct FakeHost {
    layout: Layout,
    down: KeyboardReport,
    text: [u8; TEXT_MAX],
    len: usize,
    reports: u32,
    /// A digest of every report received (which keys and modifiers were
    /// sent, not only the text they made: VER-13 compares it).
    digest: u64,
    /// Keys that went down while another non-modifier key was still held
    /// (a typist must never do that).
    pub overlaps: u32,
}

impl FakeHost {
    #[must_use]
    pub fn new(layout: Layout) -> FakeHost {
        FakeHost {
            layout,
            down: KeyboardReport::RELEASED,
            text: [0; TEXT_MAX],
            len: 0,
            reports: 0,
            digest: 0,
            overlaps: 0,
        }
    }

    /// One input report from the device.
    pub fn feed(&mut self, bytes: &[u8; 8]) {
        let r = KeyboardReport::from_bytes(bytes);
        self.reports += 1;
        self.digest =
            (self.digest ^ u64::from_le_bytes(*bytes)).wrapping_mul(0x0100_0000_01b3).rotate_left(7);
        let held = self.down.keys.iter().filter(|&&k| k != 0).count();
        for &k in r.keys.iter().filter(|&&k| k != 0) {
            if self.down.keys.contains(&k) {
                continue;
            }
            if held > 0 {
                self.overlaps += 1;
            }
            let c = layout::level_of(r.modifiers).and_then(|lv| layout::char_of(self.layout, k, lv));
            if let Some(c) = c {
                self.push(c);
            }
        }
        self.down = r;
    }

    fn push(&mut self, c: char) {
        let mut b = [0u8; 4];
        let s = c.encode_utf8(&mut b).as_bytes();
        if self.len + s.len() > TEXT_MAX {
            return;
        }
        self.text[self.len..self.len + s.len()].copy_from_slice(s);
        self.len += s.len();
    }

    /// Everything typed so far.
    #[must_use]
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// No key or modifier is held at the computer.
    #[must_use]
    pub fn all_released(&self) -> bool {
        self.down.is_released()
    }

    #[must_use]
    pub fn reports(&self) -> u32 {
        self.reports
    }

    /// The digest of the reports received since `new` (keys, modifiers
    /// and their order).
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }

    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{self, modifier};

    #[test]
    fn decodes_with_the_layout_and_ignores_shortcuts() {
        let mut h = FakeHost::new(Layout::De);
        h.feed(&KeyboardReport::key(0, 0x1C).bytes());
        h.feed(&KeyboardReport::RELEASED.bytes());
        h.feed(&KeyboardReport::key(modifier::LEFT_CTRL, 0x06).bytes());
        h.feed(&KeyboardReport::RELEASED.bytes());
        h.feed(&KeyboardReport::key(modifier::RIGHT_ALT, 0x14).bytes());
        assert_eq!(h.text(), "z@");
        assert!(!h.all_released());
        // A second key while one is held counts as an overlap.
        let mut both = KeyboardReport::key(modifier::RIGHT_ALT, 0x14);
        both.press(usage::A);
        h.feed(&both.bytes());
        assert_eq!(h.overlaps, 1);
        h.feed(&KeyboardReport::RELEASED.bytes());
        assert!(h.all_released());
        assert_eq!(h.reports(), 7);
        h.clear();
        assert_eq!(h.text(), "");
        assert_eq!(h.layout(), Layout::De);
    }
}
