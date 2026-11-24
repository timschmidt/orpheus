//! Peak meter with hold and clip (05 §7.5), and integer dBFS.

/// Hold time of the peak marker.
pub const HOLD_MS: u32 = 1500;
/// Clip indication stays this long.
pub const CLIP_MS: u32 = 2000;
/// A sample this close to full scale (−0.1 dBFS) counts as clipping.
pub const CLIP_AMP: u32 = 32_400;
/// Fall of the bar, dB per second.
pub const FALL_DB_PER_S: i32 = 24;
/// Floor of the scale.
pub const FLOOR_DB: i16 = -60;

/// log2(x) in Q8 for x ≥ 1 (x = 0 gives 0).
#[must_use]
pub fn log2_q8(x: u32) -> i32 {
    if x == 0 {
        return 0;
    }
    let msb = 31 - x.leading_zeros() as i32;
    // Mantissa in [1, 2) as Q16, then log2(1 + f) ≈ f + 0.34·f·(1 − f).
    let m = i64::from(if msb >= 16 { x >> (msb - 16) } else { x << (16 - msb) });
    let f = m - 65_536;
    let corr = f * (65_536 - f) / 65_536 * 22_282 / 65_536;
    msb * 256 + ((f + corr) / 256) as i32
}

/// Peak amplitude (0–32768) in dBFS, rounded down; `FLOOR_DB` or less for
/// silence.
#[must_use]
pub fn db(amp: u32) -> i16 {
    if amp == 0 {
        return -96;
    }
    // 20·log10(a / 32768) = 6.0206 · (log2 a − 15).
    let l = i64::from(log2_q8(amp) - 15 * 256);
    (l * 60_206).div_euclid(2_560_000).clamp(-96, 0) as i16
}

/// Amplitude for a dBFS value (inverse of [`db`], to ±1 %).
#[must_use]
pub fn amp(db: i16) -> u32 {
    // 10^(dB/20) by halving per 6.0206 dB and a linear step in between.
    let d = i32::from(db.min(0));
    let mut a: u64 = 32_768 << 16;
    let mut rest = -d * 1000;
    while rest >= 6_021 {
        a >>= 1;
        rest -= 6_021;
    }
    // Remaining fraction of 6.02 dB: × 2^(−rest/6021) ≈ 1 − 0.5·r (+ curve).
    let r = rest as u64 * 65_536 / 6_021;
    let factor = 65_536 - r / 2 + r * (65_536 - r) / 65_536 * 1_095 / 10_000;
    ((a * factor) >> 32) as u32
}

/// One channel's meter state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    /// Bar, dBFS (falls at [`FALL_DB_PER_S`]).
    pub bar: i16,
    /// Held peak, dBFS.
    pub hold: i16,
    hold_left: u32,
}

/// Levels for the screen and the status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levels {
    pub bar: [i16; 2],
    pub hold: [i16; 2],
    pub clip: bool,
    /// Peak of the last block (linear), per channel.
    pub peak: [u16; 2],
}

/// The meter.
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    ch: [Channel; 2],
    peak: [u16; 2],
    clip_left: u32,
}

impl Default for Meter {
    fn default() -> Self {
        let c = Channel { bar: FLOOR_DB, hold: FLOOR_DB, hold_left: 0 };
        Meter { ch: [c; 2], peak: [0; 2], clip_left: 0 }
    }
}

impl Meter {
    /// Feed one block (interleaved, `channels`) lasting `ms`.
    pub fn feed(&mut self, block: &[i16], channels: u8, ms: u32) {
        let ch = usize::from(channels.clamp(1, 2));
        let mut peak = [0u32; 2];
        let mut clipped = false;
        for f in block.chunks_exact(ch) {
            for (p, &s) in peak.iter_mut().zip(f) {
                let a = u32::from(s.unsigned_abs());
                *p = (*p).max(a);
                clipped |= a >= CLIP_AMP;
            }
        }
        if ch == 1 {
            peak[1] = peak[0];
        }
        let fall = (FALL_DB_PER_S * ms as i32 / 1000).max(1) as i16;
        for ((&pk, out), m) in peak.iter().zip(self.peak.iter_mut()).zip(self.ch.iter_mut()) {
            *out = pk.min(u32::from(u16::MAX)) as u16;
            let d = db(pk).max(FLOOR_DB);
            m.bar = d.max(m.bar.saturating_sub(fall)).max(FLOOR_DB);
            if d >= m.hold || m.hold_left <= ms {
                m.hold = d.max(if m.hold_left <= ms { FLOOR_DB } else { m.hold });
                if d >= m.hold {
                    m.hold_left = HOLD_MS;
                }
            } else {
                m.hold_left -= ms;
            }
        }
        if clipped {
            self.clip_left = CLIP_MS;
        } else {
            self.clip_left = self.clip_left.saturating_sub(ms);
        }
    }

    #[must_use]
    pub fn levels(&self) -> Levels {
        Levels {
            bar: [self.ch[0].bar, self.ch[1].bar],
            hold: [self.ch[0].hold, self.ch[1].hold],
            clip: self.clip_left > 0,
            peak: self.peak,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_scale() {
        assert_eq!(db(32_768), 0);
        assert_eq!(db(32_767), -1);
        assert_eq!(db(16_384), -7, "−6.02 dB rounds down");
        assert!((-21..=-20).contains(&db(3_277)));
        assert!((-41..=-40).contains(&db(328)));
        assert_eq!(db(0), -96);
        for d in [0i16, -3, -6, -12, -20, -40, -59] {
            let a = amp(d);
            let back = db(a);
            assert!((back - d).abs() <= 1, "{d} → {a} → {back}");
        }
    }

    #[test]
    fn bar_falls_and_hold_holds() {
        let mut m = Meter::default();
        let loud = [16_000i16; 64];
        let quiet = [0i16; 64];
        m.feed(&loud, 1, 10);
        let l = m.levels();
        assert_eq!(l.bar[0], l.hold[0]);
        assert!(l.bar[0] > -8);
        for _ in 0..50 {
            m.feed(&quiet, 1, 10);
        }
        let l = m.levels();
        assert!(l.bar[0] < -15, "{l:?}");
        assert!(l.hold[0] > -8, "still held after 0.5 s");
        for _ in 0..150 {
            m.feed(&quiet, 1, 10);
        }
        assert_eq!(m.levels().hold[0], FLOOR_DB);
        assert!(!m.levels().clip);
        m.feed(&[i16::MIN; 4], 1, 10);
        assert!(m.levels().clip);
    }
}
