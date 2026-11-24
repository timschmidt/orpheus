//! Shuffle as a keyed permutation of `0..n` (04 §6.5): a 4-round Feistel
//! network on the smallest even-width power-of-two domain that holds `n`,
//! with cycle walking back into `0..n`. O(1) memory for any queue length;
//! the inverse makes "previous" and "where is item X" exact; the order is
//! reproduced from the seed after a restart.

const ROUNDS: u32 = 4;

/// The permutation for one queue length and seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shuffle {
    n: u32,
    half: u32,
    keys: [u32; ROUNDS as usize],
}

/// murmur3's 32-bit finaliser.
fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

impl Shuffle {
    #[must_use]
    pub fn new(n: u32, seed: u32) -> Self {
        // Bits of the domain: at least 2, even, 2^bits >= n.
        let bits = (32 - n.saturating_sub(1).leading_zeros()).max(2);
        let bits = bits + (bits & 1);
        let mut keys = [0u32; ROUNDS as usize];
        let mut k = seed ^ 0x9E37_79B9;
        for key in &mut keys {
            k = mix(k.wrapping_add(0x6A09_E667));
            *key = k;
        }
        Self { n, half: bits / 2, keys }
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        self.n
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn mask(&self) -> u32 {
        (1u32 << self.half) - 1
    }

    fn f(&self, r: usize, x: u32) -> u32 {
        mix(x ^ self.keys[r]) & self.mask()
    }

    fn enc(&self, x: u32) -> u32 {
        let m = self.mask();
        let (mut l, mut r) = (x >> self.half, x & m);
        for i in 0..ROUNDS as usize {
            let t = l ^ self.f(i, r);
            l = r;
            r = t;
        }
        (l << self.half) | r
    }

    fn dec(&self, x: u32) -> u32 {
        let m = self.mask();
        let (mut l, mut r) = (x >> self.half, x & m);
        for i in (0..ROUNDS as usize).rev() {
            let t = r ^ self.f(i, l);
            r = l;
            l = t;
        }
        (l << self.half) | r
    }

    /// The item at play position `pos` (`pos < n`).
    #[must_use]
    pub fn item(&self, pos: u32) -> u32 {
        if pos >= self.n {
            return pos;
        }
        let mut x = pos;
        loop {
            x = self.enc(x);
            if x < self.n {
                return x;
            }
        }
    }

    /// The play position of `item` (inverse of [`Shuffle::item`]).
    #[must_use]
    pub fn pos(&self, item: u32) -> u32 {
        if item >= self.n {
            return item;
        }
        let mut x = item;
        loop {
            x = self.dec(x);
            if x < self.n {
                return x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_a_permutation_with_inverse() {
        for n in [1u32, 2, 3, 5, 16, 17, 100, 1000, 4097] {
            for seed in [0u32, 1, 0xDEAD_BEEF] {
                let s = Shuffle::new(n, seed);
                let mut seen = std::vec![false; n as usize];
                for p in 0..n {
                    let i = s.item(p);
                    assert!(i < n && !seen[i as usize]);
                    seen[i as usize] = true;
                    assert_eq!(s.pos(i), p);
                }
            }
        }
    }

    #[test]
    fn seeds_differ_and_large_queues_work() {
        let a = Shuffle::new(50_000, 1);
        let b = Shuffle::new(50_000, 2);
        let same = (0..1000).filter(|&p| a.item(p) == b.item(p)).count();
        assert!(same < 10);
        // Not the identity, and not trivially ordered.
        let ordered = (0..1000).filter(|&p| a.item(p + 1) == a.item(p) + 1).count();
        assert!(ordered < 10);
        assert_eq!(a.pos(a.item(49_999)), 49_999);
    }
}
