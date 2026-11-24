//! What the computer makes of a key: the four layouts the typist knows.
//!
//! The tables describe the computer's layout (as X11/xkb and Windows map
//! the usages), so typing a character means finding the key and level
//! that produce it there. One function, [`char_of`], is the single source:
//! [`stroke`] searches it, and the fake host ([`crate::host`]) decodes with
//! it, so typing and decoding cannot disagree.
//!
//! Characters behind dead keys (German `^` and `` ` ``, French `~` and
//! `` ` `` among them) are not produced: they would need a second stroke
//! whose result depends on the computer's settings.

use crate::usage::{self, modifier};

/// The computer's keyboard layout (Settings › USB › Keyboard layout).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Layout {
    #[default]
    Us = 0,
    Uk = 1,
    De = 2,
    Fr = 3,
}

impl Layout {
    pub const ALL: [Layout; 4] = [Layout::Us, Layout::Uk, Layout::De, Layout::Fr];

    #[must_use]
    pub const fn from_index(i: u8) -> Layout {
        match i {
            1 => Layout::Uk,
            2 => Layout::De,
            3 => Layout::Fr,
            _ => Layout::Us,
        }
    }

    /// Short name for logs and the developer channel.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Layout::Us => "us",
            Layout::Uk => "uk",
            Layout::De => "de",
            Layout::Fr => "fr",
        }
    }

    #[must_use]
    pub fn from_code(s: &[u8]) -> Option<Layout> {
        Layout::ALL.into_iter().find(|l| l.code().as_bytes() == s)
    }
}

/// Shift state of a stroke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Base,
    Shift,
    /// AltGr (right Alt).
    AltGr,
}

/// One key with its modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stroke {
    pub modifiers: u8,
    pub usage: u8,
}

const fn pick(level: Level, base: char, shift: char) -> Option<char> {
    match level {
        Level::Base => Some(base),
        Level::Shift => Some(shift),
        Level::AltGr => None,
    }
}

fn letter(level: Level, ch: u8) -> Option<char> {
    match level {
        Level::Base => Some(char::from(ch)),
        Level::Shift => Some(char::from(ch.to_ascii_uppercase())),
        Level::AltGr => None,
    }
}

/// US: the reference the others differ from.
fn us(u: u8, level: Level) -> Option<char> {
    match u {
        usage::A..=usage::Z => letter(level, b'a' + (u - usage::A)),
        usage::N1..=usage::N0 => {
            let i = usize::from(u - usage::N1);
            let base = b"1234567890"[i];
            let shift = b"!@#$%^&*()"[i];
            pick(level, char::from(base), char::from(shift))
        }
        usage::ENTER => pick(level, '\n', '\n'),
        usage::TAB => pick(level, '\t', '\t'),
        usage::SPACE => pick(level, ' ', ' '),
        usage::MINUS => pick(level, '-', '_'),
        usage::EQUAL => pick(level, '=', '+'),
        usage::LEFT_BRACKET => pick(level, '[', '{'),
        usage::RIGHT_BRACKET => pick(level, ']', '}'),
        usage::BACKSLASH | usage::NON_US_HASH => pick(level, '\\', '|'),
        usage::SEMICOLON => pick(level, ';', ':'),
        usage::APOSTROPHE => pick(level, '\'', '"'),
        usage::GRAVE => pick(level, '`', '~'),
        usage::COMMA => pick(level, ',', '<'),
        usage::DOT => pick(level, '.', '>'),
        usage::SLASH => pick(level, '/', '?'),
        _ => None,
    }
}

fn uk(u: u8, level: Level) -> Option<char> {
    match (u, level) {
        (0x1F, Level::Shift) => Some('"'),
        (0x20, Level::Shift) => Some('£'),
        (0x21, Level::AltGr) => Some('€'),
        (usage::APOSTROPHE, _) => pick(level, '\'', '@'),
        (usage::BACKSLASH | usage::NON_US_HASH, _) => pick(level, '#', '~'),
        (usage::GRAVE, _) => pick(level, '`', '¬'),
        (usage::NON_US_BACKSLASH, _) => pick(level, '\\', '|'),
        _ => us(u, level),
    }
}

fn de(u: u8, level: Level) -> Option<char> {
    if level == Level::AltGr {
        return match u {
            0x14 => Some('@'),
            0x08 => Some('€'),
            0x10 => Some('µ'),
            0x1F => Some('²'),
            0x20 => Some('³'),
            0x24 => Some('{'),
            0x25 => Some('['),
            0x26 => Some(']'),
            0x27 => Some('}'),
            usage::MINUS => Some('\\'),
            usage::RIGHT_BRACKET => Some('~'),
            usage::NON_US_BACKSLASH => Some('|'),
            _ => None,
        };
    }
    match u {
        0x1C => letter(level, b'z'),
        0x1D => letter(level, b'y'),
        usage::N1..=usage::N0 => {
            let i = usize::from(u - usage::N1);
            let shift = ['!', '"', '§', '$', '%', '&', '/', '(', ')', '='][i];
            pick(level, char::from(b"1234567890"[i]), shift)
        }
        usage::MINUS => pick(level, 'ß', '?'),
        // Dead acute and grave.
        usage::EQUAL => None,
        usage::LEFT_BRACKET => pick(level, 'ü', 'Ü'),
        usage::RIGHT_BRACKET => pick(level, '+', '*'),
        usage::BACKSLASH | usage::NON_US_HASH => pick(level, '#', '\''),
        usage::SEMICOLON => pick(level, 'ö', 'Ö'),
        usage::APOSTROPHE => pick(level, 'ä', 'Ä'),
        // Dead circumflex; degree sign with Shift.
        usage::GRAVE => (level == Level::Shift).then_some('°'),
        usage::COMMA => pick(level, ',', ';'),
        usage::DOT => pick(level, '.', ':'),
        usage::SLASH => pick(level, '-', '_'),
        usage::NON_US_BACKSLASH => pick(level, '<', '>'),
        _ => us(u, level),
    }
}

fn fr(u: u8, level: Level) -> Option<char> {
    if level == Level::AltGr {
        return match u {
            0x08 => Some('€'),
            0x20 => Some('#'),
            0x21 => Some('{'),
            0x22 => Some('['),
            0x23 => Some('|'),
            0x25 => Some('\\'),
            0x26 => Some('^'),
            0x27 => Some('@'),
            usage::MINUS => Some(']'),
            usage::EQUAL => Some('}'),
            _ => None,
        };
    }
    match u {
        0x04 => letter(level, b'q'),
        0x14 => letter(level, b'a'),
        0x1A => letter(level, b'z'),
        0x1D => letter(level, b'w'),
        usage::SEMICOLON => letter(level, b'm'),
        0x10 => pick(level, ',', '?'),
        usage::N1..=usage::N0 => {
            let i = usize::from(u - usage::N1);
            let base = ['&', 'é', '"', '\'', '(', '-', 'è', '_', 'ç', 'à'][i];
            pick(level, base, char::from(b"1234567890"[i]))
        }
        usage::MINUS => pick(level, ')', '°'),
        usage::EQUAL => pick(level, '=', '+'),
        // Dead circumflex and diaeresis.
        usage::LEFT_BRACKET => None,
        usage::RIGHT_BRACKET => pick(level, '$', '£'),
        usage::BACKSLASH | usage::NON_US_HASH => pick(level, '*', 'µ'),
        usage::APOSTROPHE => pick(level, 'ù', '%'),
        usage::GRAVE => (level == Level::Base).then_some('²'),
        usage::COMMA => pick(level, ';', '.'),
        usage::DOT => pick(level, ':', '/'),
        usage::SLASH => pick(level, '!', '§'),
        usage::NON_US_BACKSLASH => pick(level, '<', '>'),
        _ => us(u, level),
    }
}

/// The character `layout` produces for `usage` at `level`; `None` for
/// keys that type nothing (or only through a dead key). Keypad digits and
/// keypad Enter are the same on every layout (with Num Lock on).
#[must_use]
pub fn char_of(layout: Layout, usage: u8, level: Level) -> Option<char> {
    match usage {
        usage::KP_ENTER => return pick(level, '\n', '\n'),
        usage::KP1..=usage::KP0 => {
            let i = usize::from(usage - usage::KP1);
            return (level == Level::Base).then(|| char::from(b"1234567890"[i]));
        }
        _ => {}
    }
    match layout {
        Layout::Us => us(usage, level),
        Layout::Uk => uk(usage, level),
        Layout::De => de(usage, level),
        Layout::Fr => fr(usage, level),
    }
}

/// The level the computer applies for `modifiers`, or `None` when they
/// make a shortcut rather than a character (Ctrl, Alt, GUI, Shift+AltGr).
#[must_use]
pub const fn level_of(modifiers: u8) -> Option<Level> {
    let other = modifiers & !(modifier::SHIFT | modifier::RIGHT_ALT);
    if other != 0 {
        return None;
    }
    match (modifiers & modifier::SHIFT != 0, modifiers & modifier::RIGHT_ALT != 0) {
        (false, false) => Some(Level::Base),
        (true, false) => Some(Level::Shift),
        (false, true) => Some(Level::AltGr),
        (true, true) => None,
    }
}

/// Usages searched for a character, in order of preference.
const KEYS: core::ops::RangeInclusive<u8> = usage::A..=usage::SLASH;

/// The stroke that types `ch` on `layout`. Digits go to the keypad when
/// `keypad_digits` is set (layouts whose top row needs Shift, 06 §5.4).
#[must_use]
pub fn stroke(layout: Layout, ch: char, keypad_digits: bool) -> Option<Stroke> {
    if keypad_digits && let Some(d) = ch.to_digit(10) {
        return Some(Stroke { modifiers: 0, usage: usage::keypad_digit(d as u8) });
    }
    for (level, modifiers) in
        [(Level::Base, 0), (Level::Shift, modifier::LEFT_SHIFT), (Level::AltGr, modifier::RIGHT_ALT)]
    {
        for u in KEYS.chain(core::iter::once(usage::NON_US_BACKSLASH)) {
            if char_of(layout, u, level) == Some(ch) {
                return Some(Stroke { modifiers, usage: u });
            }
        }
    }
    None
}

/// The first character of `text` that `layout` cannot type.
#[must_use]
pub fn untypable(layout: Layout, text: &str, keypad_digits: bool) -> Option<char> {
    text.chars().find(|&c| stroke(layout, c, keypad_digits).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRINTABLE: core::ops::RangeInclusive<u8> = 0x20..=0x7E;

    #[test]
    fn every_ascii_character_on_us_and_uk() {
        for l in [Layout::Us, Layout::Uk] {
            for b in PRINTABLE {
                let c = char::from(b);
                let s = stroke(l, c, false).unwrap_or_else(|| panic!("{l:?} cannot type {c:?}"));
                let lv = level_of(s.modifiers).unwrap();
                assert_eq!(char_of(l, s.usage, lv), Some(c), "{l:?} {c:?}");
            }
        }
    }

    #[test]
    fn german_and_french_type_what_they_can_and_refuse_dead_keys() {
        for (l, refused) in [(Layout::De, "^`"), (Layout::Fr, "~`")] {
            for b in PRINTABLE {
                let c = char::from(b);
                match stroke(l, c, false) {
                    Some(s) => {
                        let lv = level_of(s.modifiers).unwrap();
                        assert_eq!(char_of(l, s.usage, lv), Some(c), "{l:?} {c:?}");
                    }
                    None => assert!(refused.contains(c), "{l:?} cannot type {c:?}"),
                }
            }
        }
    }

    #[test]
    fn known_positions() {
        let s = |l, c| stroke(l, c, false).unwrap();
        // QWERTZ swaps Y and Z; AZERTY moves A, Q, W, Z and M.
        assert_eq!(s(Layout::De, 'z').usage, 0x1C);
        assert_eq!(s(Layout::De, 'y').usage, 0x1D);
        assert_eq!(s(Layout::Fr, 'a').usage, 0x14);
        assert_eq!(s(Layout::Fr, 'q').usage, 0x04);
        assert_eq!(s(Layout::Fr, 'm').usage, usage::SEMICOLON);
        // French digits need Shift on the top row; the keypad never does.
        assert_eq!(s(Layout::Fr, '7'), Stroke { modifiers: modifier::LEFT_SHIFT, usage: 0x24 });
        assert_eq!(stroke(Layout::Fr, '7', true), Some(Stroke { modifiers: 0, usage: 0x5F }));
        assert_eq!(stroke(Layout::Us, '0', true), Some(Stroke { modifiers: 0, usage: usage::KP0 }));
        // AltGr on German, Shift for UK's @.
        assert_eq!(s(Layout::De, '@'), Stroke { modifiers: modifier::RIGHT_ALT, usage: 0x14 });
        assert_eq!(s(Layout::Uk, '@'), Stroke { modifiers: modifier::LEFT_SHIFT, usage: usage::APOSTROPHE });
        assert_eq!(s(Layout::Us, '\n').usage, usage::ENTER);
        assert_eq!(untypable(Layout::De, "a^b", false), Some('^'));
        assert_eq!(untypable(Layout::Us, "Hello, world!", false), None);
        assert_eq!(untypable(Layout::Us, "é", false), Some('é'));
        for l in Layout::ALL {
            assert_eq!(Layout::from_code(l.code().as_bytes()), Some(l));
            assert_eq!(Layout::from_index(l as u8), l);
        }
        assert_eq!(level_of(modifier::LEFT_CTRL), None);
        assert_eq!(level_of(modifier::RIGHT_SHIFT), Some(Level::Shift));
    }
}
