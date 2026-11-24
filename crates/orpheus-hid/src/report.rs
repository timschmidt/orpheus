//! The boot keyboard input report and the LED output report.

/// Byte 0 modifiers, byte 1 reserved, bytes 2..8 up to six pressed keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub keys: [u8; 6],
}

impl KeyboardReport {
    /// Nothing pressed.
    pub const RELEASED: KeyboardReport = KeyboardReport { modifiers: 0, keys: [0; 6] };

    /// One key with modifiers.
    #[must_use]
    pub const fn key(modifiers: u8, usage: u8) -> KeyboardReport {
        KeyboardReport { modifiers, keys: [usage, 0, 0, 0, 0, 0] }
    }

    /// Add a key; `false` when all six slots are taken (the report is
    /// unchanged) or the key is already pressed.
    pub fn press(&mut self, usage: u8) -> bool {
        if usage == 0 || self.keys.contains(&usage) {
            return false;
        }
        match self.keys.iter_mut().find(|k| **k == 0) {
            Some(k) => {
                *k = usage;
                true
            }
            None => false,
        }
    }

    /// Remove a key (the slots stay packed from the front).
    pub fn release(&mut self, usage: u8) {
        let mut out = [0u8; 6];
        let mut n = 0;
        for &k in &self.keys {
            if k != 0 && k != usage {
                out[n] = k;
                n += 1;
            }
        }
        self.keys = out;
    }

    /// No key and no modifier pressed.
    #[must_use]
    pub fn is_released(&self) -> bool {
        self.modifiers == 0 && self.keys.iter().all(|&k| k == 0)
    }

    /// The 8 bytes sent on the interrupt IN endpoint (or after report id 1
    /// on the Bluetooth module's descriptor).
    #[must_use]
    pub const fn bytes(&self) -> [u8; 8] {
        let k = self.keys;
        [self.modifiers, 0, k[0], k[1], k[2], k[3], k[4], k[5]]
    }

    /// Parse 8 bytes (the reserved byte is ignored).
    #[must_use]
    pub const fn from_bytes(b: &[u8; 8]) -> KeyboardReport {
        KeyboardReport { modifiers: b[0], keys: [b[2], b[3], b[4], b[5], b[6], b[7]] }
    }
}

/// The LED output report the computer sends (SET_REPORT on endpoint 0).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Leds(pub u8);

impl Leds {
    #[must_use]
    pub const fn num_lock(self) -> bool {
        self.0 & 1 != 0
    }
    #[must_use]
    pub const fn caps_lock(self) -> bool {
        self.0 & 2 != 0
    }
    #[must_use]
    pub const fn scroll_lock(self) -> bool {
        self.0 & 4 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_release_and_bytes() {
        let mut r = KeyboardReport::RELEASED;
        assert!(r.is_released());
        for u in 4..10 {
            assert!(r.press(u));
        }
        assert!(!r.press(11), "six slots");
        assert!(!r.press(4), "already down");
        r.release(5);
        assert_eq!(r.keys, [4, 6, 7, 8, 9, 0]);
        r.modifiers = 0x02;
        assert_eq!(r.bytes(), [0x02, 0, 4, 6, 7, 8, 9, 0]);
        assert_eq!(KeyboardReport::from_bytes(&r.bytes()), r);
        assert!(!r.is_released());
        assert!(Leds(1).num_lock() && !Leds(1).caps_lock() && Leds(6).scroll_lock());
    }
}
