//! The games' random numbers: SplitMix64 (Steele, Lea and Flood 2014; the
//! public-domain constants of Vigna's reference). One 64-bit word of state,
//! saved with the game, so a restored or replayed game draws the same
//! numbers.

/// A seedable generator. `Rng::new(seed)` with the same seed always gives
/// the same sequence, on every target.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub const fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The state, for saving ([`Rng::new`] restores it).
    #[must_use]
    pub const fn state(&self) -> u64 {
        self.0
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform-enough in `0..n` (`0` for `n <= 1`): the high word of a
    /// 32 × 32-bit product (bias below `n / 2^32`, no loop).
    pub fn below(&mut self, n: u32) -> u32 {
        if n <= 1 {
            // Still draw: the sequence must not depend on `n`.
            self.next_u32();
            return 0;
        }
        ((u64::from(self.next_u32()) * u64::from(n)) >> 32) as u32
    }

    /// True with probability `per_mille / 1000`.
    pub fn chance(&mut self, per_mille: u16) -> bool {
        self.below(1000) < u32::from(per_mille)
    }

    /// A fresh seed from anything that differs between games (the clock,
    /// the uptime): mixes the words so that close inputs give unrelated
    /// seeds.
    #[must_use]
    pub fn seed_from(a: u64, b: u64) -> u64 {
        let mut r = Rng::new(a ^ b.rotate_left(32));
        r.next_u64() ^ b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sequence() {
        // SplitMix64 from seed 0 (the reference implementation's output).
        let mut r = Rng::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        let s = r.state();
        let mut a = Rng::new(s);
        assert_eq!(a.next_u64(), r.next_u64());
    }

    #[test]
    fn below_is_in_range_and_spread() {
        let mut r = Rng::new(42);
        let mut counts = [0u32; 7];
        for _ in 0..70_000 {
            let v = r.below(7);
            assert!(v < 7);
            counts[v as usize] += 1;
        }
        for c in counts {
            assert!((9_000..11_000).contains(&c), "{counts:?}");
        }
        assert_eq!(r.below(0), 0);
        assert_eq!(r.below(1), 0);
        let hits = (0..10_000).filter(|_| r.chance(100)).count();
        assert!((800..1200).contains(&hits), "{hits}");
        assert_ne!(Rng::seed_from(1, 2), Rng::seed_from(1, 3));
    }
}
