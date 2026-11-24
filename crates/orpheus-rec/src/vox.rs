//! Voice activation (05 §7.5): blocks are written while the input is above
//! a threshold and for a hang time after it last was; otherwise the
//! recording waits (the timer does not run).

/// Thresholds offered (Settings › Recording › Voice activation): off,
/// −50, −40, −30 dBFS.
pub const THRESHOLDS_DB: [i16; 4] = [0, -50, -40, -30];
/// Hang time.
pub const HANG_MS: u32 = 2000;

/// The gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vox {
    /// Linear threshold (0–32767).
    threshold: u32,
    hang_ms: u32,
    /// Milliseconds since the input was last above the threshold.
    quiet_ms: u32,
    open: bool,
}

impl Vox {
    /// A gate at `db` dBFS (closed until the first loud block).
    #[must_use]
    pub fn new(db: i16, hang_ms: u32) -> Self {
        Vox { threshold: crate::meter::amp(db), hang_ms, quiet_ms: u32::MAX / 2, open: false }
    }

    /// A block with this peak and length: write it?
    pub fn feed(&mut self, peak: u32, ms: u32) -> bool {
        if peak >= self.threshold {
            self.quiet_ms = 0;
        } else {
            self.quiet_ms = self.quiet_ms.saturating_add(ms);
        }
        self.open = self.quiet_ms <= self.hang_ms;
        self.open
    }

    /// Writing now.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_on_sound_and_hangs() {
        let mut v = Vox::new(-40, 500);
        assert!(!v.feed(100, 20), "quiet: closed");
        assert!(v.feed(1_000, 20), "−30 dBFS: open");
        for _ in 0..25 {
            assert!(v.feed(10, 20), "hang");
        }
        assert!(!v.feed(10, 20), "closed after 500 ms");
        assert!(!v.is_open());
    }
}
