//! Keyboard/keypad usage ids (HID Usage Tables, page 0x07) and the
//! modifier bits of the boot report's first byte.

/// Modifier bits (byte 0 of the boot report).
pub mod modifier {
    pub const LEFT_CTRL: u8 = 0x01;
    pub const LEFT_SHIFT: u8 = 0x02;
    pub const LEFT_ALT: u8 = 0x04;
    pub const LEFT_GUI: u8 = 0x08;
    pub const RIGHT_CTRL: u8 = 0x10;
    pub const RIGHT_SHIFT: u8 = 0x20;
    /// AltGr on European layouts.
    pub const RIGHT_ALT: u8 = 0x40;
    pub const RIGHT_GUI: u8 = 0x80;
    /// Either shift key.
    pub const SHIFT: u8 = LEFT_SHIFT | RIGHT_SHIFT;
}

/// `a` (US position); `a`..`z` are consecutive.
pub const A: u8 = 0x04;
pub const Z: u8 = 0x1D;
/// `1`..`9` are consecutive, `0` follows `9`.
pub const N1: u8 = 0x1E;
pub const N0: u8 = 0x27;
pub const ENTER: u8 = 0x28;
pub const ESCAPE: u8 = 0x29;
pub const BACKSPACE: u8 = 0x2A;
pub const TAB: u8 = 0x2B;
pub const SPACE: u8 = 0x2C;
pub const MINUS: u8 = 0x2D;
pub const EQUAL: u8 = 0x2E;
pub const LEFT_BRACKET: u8 = 0x2F;
pub const RIGHT_BRACKET: u8 = 0x30;
pub const BACKSLASH: u8 = 0x31;
/// ISO key left of Enter (`#~` on UK keyboards).
pub const NON_US_HASH: u8 = 0x32;
pub const SEMICOLON: u8 = 0x33;
pub const APOSTROPHE: u8 = 0x34;
pub const GRAVE: u8 = 0x35;
pub const COMMA: u8 = 0x36;
pub const DOT: u8 = 0x37;
pub const SLASH: u8 = 0x38;
pub const F5: u8 = 0x3E;
pub const PAGE_UP: u8 = 0x4B;
pub const PAGE_DOWN: u8 = 0x4E;
pub const RIGHT: u8 = 0x4F;
pub const LEFT: u8 = 0x50;
pub const DOWN: u8 = 0x51;
pub const UP: u8 = 0x52;
pub const NUM_LOCK: u8 = 0x53;
pub const KP_ENTER: u8 = 0x58;
/// Keypad `1`..`9` are consecutive, keypad `0` follows keypad `9`.
pub const KP1: u8 = 0x59;
pub const KP0: u8 = 0x62;
/// ISO key right of the left shift (`\|` on UK, `<>` on German and
/// French keyboards).
pub const NON_US_BACKSLASH: u8 = 0x64;
/// Highest usage the boot descriptor's key array declares.
pub const MAX_BOOT: u8 = 0x65;

/// The keypad usage of digit `d` (0..=9).
#[must_use]
pub const fn keypad_digit(d: u8) -> u8 {
    if d == 0 { KP0 } else { KP1 + d - 1 }
}

/// The top-row usage of digit `d` (0..=9).
#[must_use]
pub const fn top_row_digit(d: u8) -> u8 {
    if d == 0 { N0 } else { N1 + d - 1 }
}
