//! Channel mode and balance as one 2 × 2 matrix, ramped per frame so a mode
//! change crossfades instead of switching.
//!
//! The mode is applied first, then the balance: `M = diag(bL, bR) · Mode`.
//! (Balance before a mono down-mix would do nothing audible; this way
//! "mono + balance" pans the mono signal.)
//!
//! Exact matrices (stereo, swap, left/right only) are bit-exact: the
//! coefficients are 1.0 and 0.0 in Q3.28, and mono is `(L + R) / 2` rounded
//! down.

use crate::fixed::{Ramp, UNITY};

/// Ramp length of a mode or balance change.
pub const MATRIX_RAMP_MS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMode {
    #[default]
    Stereo,
    /// (L + R) / 2 on both sides.
    Mono,
    /// Left and right exchanged.
    Swap,
    /// The left channel on both sides.
    LeftOnly,
    /// The right channel on both sides.
    RightOnly,
}

impl ChannelMode {
    /// `[ll, lr, rl, rr]`: L' = ll·L + lr·R, R' = rl·L + rr·R.
    #[must_use]
    pub const fn matrix(self) -> [i32; 4] {
        const H: i32 = UNITY / 2;
        match self {
            Self::Stereo => [UNITY, 0, 0, UNITY],
            Self::Mono => [H, H, H, H],
            Self::Swap => [0, UNITY, UNITY, 0],
            Self::LeftOnly => [UNITY, 0, UNITY, 0],
            Self::RightOnly => [0, UNITY, 0, UNITY],
        }
    }
}

/// Balance gains for `balance` in −100 (left only) … +100 (right only):
/// the far side is attenuated linearly in amplitude, the near side stays.
#[must_use]
pub fn balance_gains(balance: i8) -> [i32; 2] {
    let b = i64::from(balance.clamp(-100, 100));
    let one = i64::from(UNITY);
    let l = if b > 0 { one * (100 - b) / 100 } else { one };
    let r = if b < 0 { one * (100 + b) / 100 } else { one };
    [l as i32, r as i32]
}

const IDENT: [i32; 4] = [UNITY, 0, 0, UNITY];

#[derive(Debug, Clone)]
pub struct Matrix {
    mode: ChannelMode,
    balance: i8,
    rate: u32,
    ramp: Ramp<4>,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::new()
    }
}

impl Matrix {
    #[must_use]
    pub const fn new() -> Self {
        Self { mode: ChannelMode::Stereo, balance: 0, rate: 44100, ramp: Ramp::new(IDENT) }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
    }

    fn target(&self) -> [i32; 4] {
        let m = self.mode.matrix();
        let [bl, br] = balance_gains(self.balance);
        let s = |g: i32, v: i32| ((i64::from(g) * i64::from(v)) >> 28) as i32;
        [s(bl, m[0]), s(bl, m[1]), s(br, m[2]), s(br, m[3])]
    }

    fn retarget(&mut self) {
        let t = self.target();
        self.ramp.to(t, self.rate * MATRIX_RAMP_MS / 1000);
    }

    pub fn set_mode(&mut self, mode: ChannelMode) {
        if mode != self.mode {
            self.mode = mode;
            self.retarget();
        }
    }

    pub fn set_balance(&mut self, balance: i8) {
        let balance = balance.clamp(-100, 100);
        if balance != self.balance {
            self.balance = balance;
            self.retarget();
        }
    }

    #[must_use]
    pub fn mode(&self) -> ChannelMode {
        self.mode
    }

    #[must_use]
    pub fn balance(&self) -> i8 {
        self.balance
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.ramp.ramping() || self.ramp.cur != IDENT
    }

    /// No ramp moves: [`Self::is_active`] stays as it is until a setter.
    #[must_use]
    pub fn settled(&self) -> bool {
        !self.ramp.ramping()
    }

    /// Stereo only (`buf` interleaved L, R).
    pub fn process(&mut self, buf: &mut [i32]) {
        let (frames, _) = buf.as_chunks_mut::<2>();
        let ramping = self.ramp.ramping();
        let mut m = self.ramp.cur;
        for f in frames.iter_mut() {
            if ramping {
                m = self.ramp.tick();
            }
            let (l, r) = (i64::from(f[0]), i64::from(f[1]));
            f[0] = ((l * i64::from(m[0]) + r * i64::from(m[1])) >> 28) as i32;
            f[1] = ((l * i64::from(m[2]) + r * i64::from(m[3])) >> 28) as i32;
        }
    }

    pub fn reset(&mut self) {
        let t = self.ramp.target;
        self.ramp.jump(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_modes() {
        let mut m = Matrix::new();
        m.set_rate(1000); // 30-frame ramp
        m.set_mode(ChannelMode::Swap);
        let mut buf = [0i32; 200];
        for _ in 0..2 {
            buf = core::array::from_fn(|i| (i as i32 - 100) * 1_000_003);
            m.process(&mut buf);
        }
        for i in (0..200).step_by(2) {
            assert_eq!(buf[i], (i as i32 + 1 - 100) * 1_000_003);
            assert_eq!(buf[i + 1], (i as i32 - 100) * 1_000_003);
        }
        assert_eq!(balance_gains(50), [UNITY / 2, UNITY]);
        assert_eq!(balance_gains(-100), [UNITY, 0]);
    }
}
