//! Wrong-PIN delays: 1 s after the first failure, doubling, at most 5 min
//! (06-connectivity §5.2). The failure count lives in the vault sector and
//! is recorded *before* each attempt, so a reset or a power cut during the
//! key derivation counts as a failure.

/// Longest delay, seconds.
pub const MAX_DELAY_S: u32 = 300;

/// Seconds to wait before the next attempt after `failures` consecutive
/// failures: 0, 1, 2, 4, … 256, then 300.
#[must_use]
pub const fn delay_s(failures: u32) -> u32 {
    if failures == 0 {
        return 0;
    }
    let shift = if failures - 1 > 9 { 9 } else { failures - 1 };
    let d = 1u32 << shift;
    if d > MAX_DELAY_S { MAX_DELAY_S } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule() {
        let got: [u32; 13] = core::array::from_fn(|i| delay_s(i as u32));
        assert_eq!(got, [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
        assert_eq!(delay_s(u32::MAX), 300);
    }
}
