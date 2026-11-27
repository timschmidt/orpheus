//! Adversarial settings records and references (VER-3, 07 §3.7): every
//! value the settings store on NOR can hand back (any length, any i16)
//! for every setting; stepping from every valid value; packed items with
//! any word; and file references from damaged files. A stored value never
//! reaches the UI unvalidated, stepping never leaves the valid set, and
//! formatting never panics.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{config, edges};
use orpheus_apps::browse::FileRef;
use orpheus_apps::settings::{Kind, SettingsCache, all, decode, format_value};
use orpheus_apps::strings::Lang;
use orpheus_ui::text::StrBuf;
use proptest::prelude::*;

/// Every value a record can decode to, per kind: the edges of i16 and of
/// the item's own range.
fn values(k: &Kind, default: i16) -> Vec<i16> {
    let mut v: Vec<i16> =
        [i16::MIN, i16::MIN + 1, -2, -1, 0, 1, 2, 3, i16::MAX - 1, i16::MAX, default].to_vec();
    match *k {
        Kind::Choice(c) => {
            let n = c.len() as i16;
            v.extend([n - 1, n, n + 1]);
        }
        Kind::Range { min, max, step, .. } => {
            v.extend([
                min.saturating_sub(1),
                min,
                min.saturating_add(1),
                max.saturating_sub(step),
                max.saturating_sub(1),
                max,
                max.saturating_add(1),
            ]);
        }
        Kind::HomeApps => v.extend([0x3FFF, 0x7FFF, -0x8000]),
        _ => {}
    }
    v
}

#[test]
fn every_setting_every_record() {
    for s in all() {
        for v in values(&s.kind, s.default) {
            for bytes in [&v.to_le_bytes()[..], &[], &v.to_le_bytes()[..1], &[0xFF, 0xFF, 0xFF]] {
                let d = decode(s.id, bytes).expect("a known id decodes");
                assert_eq!(s.validate(d), d, "{:#06x}: decoded {d} is not valid", s.id.0);
                let mut out = StrBuf::<64>::new();
                format_value(s, d, Lang::ENGLISH, &mut out);
                format_value(s, v, Lang::ENGLISH, &mut out);
            }
        }
    }
}

#[test]
fn stepping_stays_valid() {
    // From every valid value of every choice and range, both ways: the
    // result is valid (a clamp to a maximum off the step grid would be
    // refused by `validate` and reset to the default).
    for s in all() {
        let valid: Vec<i16> = match s.kind {
            Kind::Toggle => vec![0, 1],
            Kind::Choice(c) => (0..c.len() as i16).collect(),
            Kind::Range { min, max, step, .. } => (min..=max).step_by(step.max(1) as usize).collect(),
            _ => continue,
        };
        for &v in &valid {
            for dir in [1i16, -1] {
                let n = s.step(v, dir);
                assert_eq!(s.validate(n), n, "{:#06x}: step {dir} from {v} gives {n}, not valid", s.id.0);
            }
        }
        if let Kind::Range { min, max, step, .. } = s.kind {
            assert_eq!(
                (i32::from(max) - i32::from(min)) % i32::from(step.max(1)),
                0,
                "{:#06x}: max off the step grid",
                s.id.0
            );
        }
    }
}

#[test]
fn cache_loads_any_record() {
    let mut c = SettingsCache::new();
    for s in all() {
        for v in values(&s.kind, s.default) {
            c.load(s.id.0, &v.to_le_bytes());
            let got = c.get(s.id);
            assert_eq!(s.validate(got), got, "{:#06x}: cached {got} after loading {v}", s.id.0);
        }
    }
    // Unknown keys are ignored.
    for k in [0u16, 1, 0x00FF, 0x1200, 0xFFFF] {
        c.load(k, &[1, 2]);
    }
    let _: Vec<_> = c.take_dirty().collect();
}

#[test]
fn file_refs_from_any_bytes() {
    for n in 0..12 {
        let b: Vec<u8> = (0..n).map(|i| 0xF0 | i as u8).collect();
        let r = FileRef::from_bytes(&b);
        if n >= 8 {
            assert_eq!(FileRef::from_bytes(&r.to_bytes()), r);
        }
    }
    for v in edges(32, 0) {
        let r = FileRef { dir: v as u32, index: v as u32 };
        assert_eq!(FileRef::from_bytes(&r.to_bytes()), r);
    }
}

/// VER-3, 2026-11-27: `Setting::step` added `dir * step` in `i16`; for an
/// item whose range reaches the `i16` limits (the packed accessibility
/// word 0x0D60 and the screen reader's word 0x0D61, `i16::MIN..=i16::MAX`)
/// a step up from 32767 or down from −32768 panicked with "attempt to add
/// with overflow" (host builds) or wrapped round to the other end of the
/// range (firmware, overflow checks off). Kept input:
/// `tests/regress/apps_settings/step-at-i16-limits` (id, value, direction
/// per line).
#[test]
fn regress_setting_step_overflow() {
    use orpheus_apps::settings::{SettingId, def};
    let path = adversarial::root().join("tests/regress/apps_settings/step-at-i16-limits");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut n = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let id = SettingId(u16::from_str_radix(f[0].trim_start_matches("0x"), 16).unwrap());
        let (v, dir): (i16, i16) = (f[1].parse().unwrap(), f[2].parse().unwrap());
        let Some(s) = def(id) else { continue };
        let got = s.step(v, dir);
        assert_eq!(got, v, "{line}: a step past the end of the range stays at the end");
        n += 1;
    }
    assert!(n >= 2);
}

/// VER-3, 2026-11-27: `format_value` showed a `Quarters` value (Speech
/// rate, 0x1005, with the screen reader built in) as `|v| * 25` in
/// `u16`: any value above 2,621 or below −2,621 (a value `format_value`
/// was handed unvalidated) panicked with "attempt to multiply with
/// overflow" (host builds) or showed a wrapped speed (firmware). Kept
/// input: `tests/regress/apps_settings/format-quarters-out-of-range` (id,
/// value per line).
#[cfg(feature = "screen-reader")]
#[test]
fn regress_speech_rate_format_overflow() {
    use orpheus_apps::settings::{SettingId, def};
    let path = adversarial::root().join("tests/regress/apps_settings/format-quarters-out-of-range");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut n = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let id = SettingId(u16::from_str_radix(f[0].trim_start_matches("0x"), 16).unwrap());
        let v: i16 = f[1].parse().unwrap();
        let s = def(id).expect("a known setting");
        let mut out = StrBuf::<64>::new();
        format_value(s, v, Lang::ENGLISH, &mut out);
        assert!(!out.as_str().is_empty(), "{line}: shown as nothing");
        n += 1;
    }
    assert!(n >= 3);
}

proptest! {
    #![proptest_config(config("apps-settings-records", 512))]

    /// Any record sequence into the cache, then sets with any values: every
    /// read is valid, packed items included.
    #[test]
    fn any_records_and_sets(recs in prop::collection::vec((any::<u16>(), any::<i16>()), 0..64),
                            sets in prop::collection::vec((0usize..400, any::<i16>()), 0..64)) {
        let ids: Vec<_> = all().map(|s| s.id).collect();
        let mut c = SettingsCache::new();
        for (k, v) in recs {
            // Half of them real keys.
            let key = if k & 1 == 0 { ids[usize::from(k >> 1) % ids.len()].0 } else { k };
            c.load(key, &v.to_le_bytes());
        }
        for (i, v) in sets {
            let _ = c.set(ids[i % ids.len()], v);
        }
        for s in all() {
            let got = c.get(s.id);
            prop_assert_eq!(s.validate(got), got);
        }
    }
}
