//! Every character the demo screens draw is covered by the built-in fonts
//! or by the committed Unifont subset (so no golden image shows tofu).
//! When this fails after new sample text, regenerate the subset:
//! `python3 sim/assets/make_subset.py unifont-17.0.05.hex sim/src crates/yoton-apps/src > sim/assets/unifont-subset.hex`.

use yoton_sim::demo::all_strings;
use yoton_sim::fallback::HexFont;
use yoton_ui::font::{UI, UI_BOLD};

#[test]
fn sample_text_is_covered() {
    let sub = HexFont::subset();
    assert!(!sub.is_empty());
    let mut missing = Vec::new();
    let mut all: Vec<&str> = all_strings();
    all.extend(yoton_apps::strings::EN.iter().copied());
    all.extend(yoton_sim::packs::RU.iter().map(|(_, t)| *t));
    all.extend(yoton_sim::packs::JA.iter().map(|(_, t)| *t));
    for s in all {
        for c in s.chars() {
            if UI.glyph(c).is_some() || UI_BOLD.glyph(c).is_some() {
                continue;
            }
            let ok = u16::try_from(u32::from(c)).is_ok_and(|cp| sub.has(cp));
            if !ok && !missing.contains(&c) {
                missing.push(c);
            }
        }
    }
    assert!(missing.is_empty(), "no glyph for {missing:?}");
}
