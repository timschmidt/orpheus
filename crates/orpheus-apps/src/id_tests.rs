//! Numbers that must stay unique across screens and services, read from
//! the sources so that a new constant is checked without being listed.

extern crate std;
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

/// `(name, value)` of every `pub const NAME: TYPE = …;` in `src` between
/// the line `start` and the end of its block (the first line that is
/// exactly `}`; the whole file for an empty `start`), the value read from
/// the last number on the line (`ScreenId(40)`, `SettingId(0x0D50)`, `8`).
fn consts(src: &str, start: &str, ty: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut inside = start.is_empty();
    for line in src.lines() {
        if !inside {
            inside = line.trim_end() == start;
            continue;
        }
        if !start.is_empty() && line == "}" {
            break;
        }
        let Some(rest) = line.trim().strip_prefix("pub const ") else { continue };
        let Some((name, rest)) = rest.split_once(':') else { continue };
        let Some((t, value)) = rest.split_once('=') else { continue };
        if t.trim() != ty {
            continue;
        }
        let v = value.trim().trim_end_matches(';').trim_end_matches(')');
        let v = v.rsplit('(').next().unwrap_or(v).trim().replace('_', "");
        let n = match v.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16),
            None => v.parse(),
        };
        out.push((String::from(name.trim()), n.unwrap_or_else(|_| panic!("cannot read {line}"))));
    }
    out
}

fn assert_unique(what: &str, items: &[(String, u32)], at_least: usize) {
    assert!(items.len() >= at_least, "{what}: only {} found", items.len());
    let mut seen = BTreeMap::new();
    for (name, v) in items {
        if let Some(other) = seen.insert(*v, name) {
            panic!("{what}: {name} and {other} are both {v:#x}");
        }
    }
}

/// Regression: two service event kinds had the same number after a merge
/// (2026-11-24, found while merging USB-6: AUTH and RECORDER), so one app's
/// events went to the other's screen; the M7 merge had the same with screen
/// ids and with the radio frequency setting (0x0D20, the step goal). Every
/// `other::` kind, `data::` tag, screen id and named setting id is unique,
/// and every named setting id has a definition.
#[test]
fn regress_service_kinds_screen_ids_setting_ids_unique() {
    let services = include_str!("services.rs");
    assert_unique("other::", &consts(services, "pub mod other {", "u16"), 8);
    assert_unique("data::", &consts(services, "pub mod data {", "u16"), 6);
    assert_unique("screen ids", &consts(include_str!("screens/mod.rs"), "", "ScreenId"), 40);
    let ids = consts(include_str!("settings.rs"), "pub mod id {", "SettingId");
    assert_unique("settings::id", &ids, 40);
    for (name, v) in &ids {
        let id = crate::settings::SettingId(u16::try_from(*v).unwrap());
        assert!(crate::settings::def(id).is_some(), "settings::id::{name} ({v:#06x}) has no definition");
    }
}
