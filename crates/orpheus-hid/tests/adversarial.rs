//! Adversarial text for the USB keyboard (VER-3, 07 §3.7): the typist
//! types codes, and over the developer channel (`kbd type`) any text, in
//! any layout. Whatever the text — control characters, characters no
//! layout has, combining marks, very long — `check` names an untypable
//! character or the computer (the fake host) receives exactly the text;
//! every press is followed by a release, keys never overlap, nothing is
//! left held; an abort at any point releases everything; layout codes of
//! any bytes are refused or name a layout.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{config, hostile_names};
use orpheus_hid::host::{FakeHost, TEXT_MAX};
use orpheus_hid::layout::{Layout, untypable};
use orpheus_hid::typist::{Typist, TypistConfig};
use proptest::prelude::*;

fn type_all(layout: Layout, keypad: bool, delay: u16, text: &str, abort_at: Option<usize>) {
    let mut t = Typist::new(TypistConfig { layout, keypad_digits: keypad, delay_ms: delay });
    assert!(t.config().delay_ms >= 2 && t.config().delay_ms <= 500);
    let mut host = FakeHost::new(layout);
    let typable = t.check(text).is_none();
    assert_eq!(typable, untypable(layout, text, keypad).is_none());
    let mut n = 0;
    while let Some(o) = t.next(text) {
        host.feed(&o.report.bytes());
        n += 1;
        if abort_at == Some(n) {
            if let Some(r) = t.abort() {
                host.feed(&r.bytes());
            }
            assert!(t.next(text).is_none() && t.is_done());
            break;
        }
        assert!(n <= 2 * text.chars().count(), "more reports than two per character");
    }
    assert!(host.all_released(), "a key left held");
    assert_eq!(host.overlaps, 0);
    if typable && abort_at.is_none() && text.len() <= TEXT_MAX {
        assert_eq!(host.text(), text, "{layout:?}: typed differently");
    }
}

#[test]
fn hostile_text_in_every_layout() {
    let mut texts = hostile_names();
    texts.extend(
        ["", "0", "123456", "12345678", "\u{0}\u{8}\u{7F}", "\t\n\r", "^`´~¨", "ßäöüÄÖÜ€§°", "@#[]{}|\\"]
            .map(String::from),
    );
    for layout in Layout::ALL {
        for keypad in [false, true] {
            for t in &texts {
                for delay in [0, 1, 12, 500, u16::MAX] {
                    type_all(layout, keypad, delay, t, None);
                }
                for at in [1, 2, 3] {
                    type_all(layout, keypad, 12, t, Some(at));
                }
            }
        }
    }
}

#[test]
fn layout_codes_of_any_bytes() {
    for a in 0..=255u8 {
        for b in [0u8, b'e', b's', b'k', b'r', b'S', 0xFF] {
            let _ = Layout::from_code(&[a, b]);
            let _ = Layout::from_code(&[a]);
        }
    }
    for c in [&b""[..], b"us", b"US", b"uk", b"de", b"fr", b"usx", b"u\0", &[0xFF; 300][..]] {
        let _ = Layout::from_code(c);
    }
}

proptest! {
    #![proptest_config(config("hid-any-text", 512))]

    /// Any text, any layout, any abort point.
    #[test]
    fn any_text(text in "\\PC{0,64}|[ -~]{0,64}|[0-9]{0,12}", layout in 0usize..4, keypad in any::<bool>(),
                abort in prop::option::of(1usize..40)) {
        type_all(Layout::ALL[layout], keypad, 12, &text, abort);
    }
}
