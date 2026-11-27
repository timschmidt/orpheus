//! Adversarial radio data (VER-3, 07 §3.7): RDS groups arrive over the
//! air from any transmitter (or noise the tuner's error flags miss), and
//! the preset list is read back from the settings store. Any group
//! sequence keeps the service name and radio text within their lengths
//! and the clock plausible; text converts to UTF-8 into any buffer whole
//! characters only; a preset list of any bytes loads only plausible
//! entries.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::config;
use orpheus_fm::presets::{BYTES, MAX, NAME, Presets};
use orpheus_fm::rds::{PS_LEN, RT_LEN, Rds, group_0a, group_2a, group_4a, to_utf8};
use proptest::prelude::*;

fn check(r: &Rds) {
    assert!(r.ps().len() <= PS_LEN);
    assert!(r.rt().len() <= RT_LEN);
    if let Some(t) = r.clock() {
        // MJD limits: after 1970, before the 17-bit MJD runs out.
        assert!(t < (1 << 17) * 86_400);
    }
    for n in [0usize, 1, 2, 3, 7, 200] {
        let mut out = vec![0u8; n];
        let w = to_utf8(r.rt(), &mut out);
        assert!(w <= n);
        assert!(core::str::from_utf8(&out[..w]).is_ok(), "cut inside a character");
        let w = to_utf8(r.ps(), &mut out);
        assert!(core::str::from_utf8(&out[..w]).is_ok());
    }
}

#[test]
fn hostile_groups() {
    let mut r = Rds::default();
    // Every group type and version with extreme payloads.
    for b in (0..=0xFFFFu16).step_by(97) {
        for (c, d) in [(0u16, 0u16), (0xFFFF, 0xFFFF), (0x0D0D, 0x0D0D), (0x8080, 0xFFFE)] {
            for bler in [0u8, 1, 2, 3, 0xFF] {
                r.feed([0x1234, b, c, d], bler, bler & 1);
                check(&r);
            }
        }
    }
    // The station changing every group, the text A/B flag flipping every
    // segment, clock groups at every edge.
    for i in 0..4000u32 {
        let pi = if i % 3 == 0 { 0x1234 } else { (i as u16).wrapping_mul(7919) };
        r.feed(group_0a(pi, (i % 4) as u8, b"\xFF\x0D\x00 \x7FABC"), 0, 0);
        r.feed(group_2a(pi, (i % 16) as u8, i % 2 == 0, &[0xFF; RT_LEN]), 0, 0);
        r.feed(group_4a(pi, u64::from(i) * 86_400 * 4000, (i % 48) as i8 - 24), 0, 0);
        r.feed(group_4a(pi, u64::MAX, i8::MIN), 0, 0);
        check(&r);
    }
    r.reset();
    check(&r);
}

#[test]
fn preset_list_of_any_bytes() {
    for fill in [0x00u8, 0x01, 0x7F, 0xFF] {
        for n in [0usize, 1, 2, 3, BYTES - 1, BYTES] {
            let mut p = Presets::new();
            p.raw_mut().fill(fill);
            p.raw_mut()[0] = 1;
            p.raw_mut()[1] = fill;
            p.loaded(n);
            assert!(p.len() <= MAX);
            for i in 0..=MAX {
                if let Some(e) = p.get(i) {
                    assert!((64_000..=108_000).contains(&e.khz) && e.name().len() <= NAME);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(config("fm-rds-groups", 256))]

    /// Any groups with any error levels.
    #[test]
    fn any_groups(groups in prop::collection::vec((prop::array::uniform4(any::<u16>()), 0u8..4, 0u8..4), 0..300)) {
        let mut r = Rds::default();
        for (g, a, b) in groups {
            r.feed(g, a, b);
            check(&r);
        }
    }

    /// Any preset bytes.
    #[test]
    fn any_presets(data in prop::collection::vec(any::<u8>(), BYTES), n in 0usize..=BYTES) {
        let mut p = Presets::new();
        p.raw_mut().copy_from_slice(&data);
        p.loaded(n);
        prop_assert!(p.len() <= MAX);
        for i in 0..p.len() {
            let e = p.get(i).unwrap();
            prop_assert!((64_000..=108_000).contains(&e.khz) && e.name().len() <= NAME);
        }
        let _ = p.neighbour(87_500, true);
        let _ = p.find(100_000);
        prop_assert!(p.save(99_900, b"x").is_some() || p.len() == MAX);
    }
}
