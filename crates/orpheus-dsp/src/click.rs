//! The key click (05 §3: "every press may produce a key click (off by
//! default) mixed into the audio output; never when recording from the
//! microphone"; 04 §3.3: mixed in software only while it sounds).
//!
//! A 3 ms burst of a 2.5 kHz triangle under a linear decay, about −15 dBFS
//! at its start: short enough to sit on top of music without masking it,
//! with no table (a triangle and a ramp are two multiplies a sample). The
//! click is a function of its frame index, so the player keeps only that
//! index (`at`): the frame of the click the next output frame gets, or
//! anything at or past [`frames`] when none sounds.
//!
//! Mixing adds the click to both channels and saturates; with no click
//! sounding the buffer is not touched.

/// Length of the click.
pub const CLICK_MS: u32 = 3;
/// Pitch of the triangle.
pub const CLICK_HZ: u32 = 2500;
/// Peak level of the first period (`i16` full scale is 32 767): ≈ −15 dBFS.
pub const CLICK_PEAK: i32 = 5800;

/// Frames the click lasts at `rate` (at least one).
#[must_use]
pub const fn frames(rate: u32) -> u32 {
    let n = rate / 1000 * CLICK_MS;
    if n == 0 { 1 } else { n }
}

/// Phase step per frame (a full period is 2³²) at `rate`.
fn step(rate: u32) -> u32 {
    ((u64::from(CLICK_HZ) << 32) / u64::from(rate.max(1))) as u32
}

/// Frame `i` of the click as `i16` level, given the phase step `st` and
/// the length `n` (0 at and past the end).
fn level(i: u32, st: u32, n: u32) -> i32 {
    if i >= n {
        return 0;
    }
    let ph = i.wrapping_mul(st);
    // Triangle from −2¹⁵ to 2¹⁵ (rising in the first half of the period).
    let p = (ph >> 16) as i32;
    let tri = if p < 0x8000 { 2 * p - 0x8000 } else { 0x17FFF - 2 * p };
    // Linear decay from the peak to 0 at the end.
    let env = CLICK_PEAK * (n - i) as i32 / n as i32;
    (tri * env) >> 15
}

/// The click alone: frame `i` at `rate` (the idle player sounds it with
/// the DAC started for it).
#[must_use]
pub fn sample(i: u32, rate: u32) -> i16 {
    level(i, step(rate), frames(rate)) as i16
}

/// Mix the click from frame `*at` into interleaved `i16` samples of `ch`
/// channels at `rate`, advancing `*at`. Nothing happens when no click
/// sounds (`*at >= frames(rate)`).
pub fn mix_i16(buf: &mut [i16], ch: usize, rate: u32, at: &mut u32) {
    let n = frames(rate);
    if *at >= n {
        return;
    }
    let st = step(rate);
    for f in buf.chunks_exact_mut(ch.max(1)) {
        if *at >= n {
            break;
        }
        let v = level(*at, st, n);
        for x in f {
            *x = (i32::from(*x) + v).clamp(-32768, 32767) as i16;
        }
        *at += 1;
    }
}

/// [`mix_i16`] for left-justified `i32` samples (24-bit in the top bits).
pub fn mix_i32(buf: &mut [i32], ch: usize, rate: u32, at: &mut u32) {
    let n = frames(rate);
    if *at >= n {
        return;
    }
    let st = step(rate);
    for f in buf.chunks_exact_mut(ch.max(1)) {
        if *at >= n {
            break;
        }
        let v = level(*at, st, n) << 16;
        for x in f {
            *x = x.saturating_add(v);
        }
        *at += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn a_short_decaying_burst() {
        for rate in [8000, 22_050, 44_100, 48_000] {
            let n = frames(rate);
            assert_eq!(n, rate / 1000 * CLICK_MS);
            let c: Vec<i16> = (0..n + 10).map(|i| sample(i, rate)).collect();
            assert!(c[n as usize..].iter().all(|&x| x == 0), "{rate}: silent after {n} frames");
            let peak = c.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0);
            assert!((4000..=5800).contains(&peak), "{rate}: peak {peak}");
            // The first third louder than the last.
            let energy = |s: &[i16]| s.iter().map(|&x| i64::from(x) * i64::from(x)).sum::<i64>();
            let t = (n / 3) as usize;
            assert!(energy(&c[..t]) > 4 * energy(&c[2 * t..n as usize]), "{rate}: decays");
            // Both signs (a tone, not a step: no DC thump).
            assert!(c.iter().any(|&x| x > 1000) && c.iter().any(|&x| x < -1000));
        }
    }

    #[test]
    fn mixing_adds_the_click_once_and_saturates() {
        let rate = 44_100;
        let n = frames(rate) as usize;
        // Stereo, in two pieces: the same as the click alone.
        let mut buf = vec![0i16; 2 * (n + 20)];
        let mut at = 0;
        let (a, b) = buf.split_at_mut(2 * 50);
        mix_i16(a, 2, rate, &mut at);
        assert_eq!(at, 50);
        mix_i16(b, 2, rate, &mut at);
        assert_eq!(at as usize, n);
        for i in 0..n + 20 {
            let want = if i < n { sample(i as u32, rate) } else { 0 };
            assert_eq!((buf[2 * i], buf[2 * i + 1]), (want, want), "frame {i}");
        }
        // Finished: nothing more is touched.
        let mut quiet = vec![7i16; 64];
        mix_i16(&mut quiet, 2, rate, &mut at);
        assert!(quiet.iter().all(|&x| x == 7));
        // Saturation at full scale.
        let mut loud = vec![i16::MAX; 2 * n];
        let mut at = 0;
        mix_i16(&mut loud, 2, rate, &mut at);
        assert!(loud.iter().all(|&x| x >= i16::MAX - CLICK_PEAK as i16));
        // 24-bit: the same click in the top bits.
        let mut wide = vec![0i32; n];
        let mut at = 0;
        mix_i32(&mut wide, 1, rate, &mut at);
        for (i, &x) in wide.iter().enumerate() {
            assert_eq!(x >> 16, i32::from(sample(i as u32, rate)), "frame {i}");
        }
    }
}
