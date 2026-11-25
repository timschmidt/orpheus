//! The Books app's model (05 §7.15, BOOK-1; feature `books`): its
//! settings, the reading faces and their metrics, the page geometry, the
//! palettes, and the service the screens use. The reader core (formats,
//! layout, the page index, card work in steps) is `orpheus-books`; the
//! screens are `screens::books`.

pub use orpheus_books as core;
use orpheus_books::layout::{Metrics, Params};
use orpheus_books::norm::{BOLD, ITALIC};
use orpheus_books::shelf::{Req, Scope, Shelf};
use orpheus_ui::color::{Color, rgb};
use orpheus_ui::font::{BOOK16, BOOK20, Font, Style, UI, UI_BOLD};
use orpheus_ui::gfx::Gfx;
use orpheus_ui::theme::Role;

use crate::services::SettingsService;
use crate::settings::{Choice, Setting, Unit, c, r, t};
use crate::strings::Str;

/// The reading settings' ids. Stored as one word ([`id::WORD`]); each
/// reads and writes under its own key.
pub mod id {
    use crate::settings::SettingId;
    pub const FONT_SIZE: SettingId = SettingId(0x0F01);
    pub const MARGINS: SettingId = SettingId(0x0F02);
    pub const SPACING: SettingId = SettingId(0x0F03);
    pub const JUSTIFY: SettingId = SettingId(0x0F04);
    pub const PALETTE: SettingId = SettingId(0x0F05);
    pub const AUTO_TURN: SettingId = SettingId(0x0F06);
    pub const FIND_IN: SettingId = SettingId(0x0F07);
    /// The stored word.
    pub const WORD: SettingId = SettingId(0x0F80);
}

const SIZES: &[Choice] = &[
    Choice::S(Str::BooksSmall),
    Choice::S(Str::Medium),
    Choice::S(Str::BooksLarge),
    Choice::S(Str::BooksExtraLarge),
];
const MARGINS: &[Choice] =
    &[Choice::S(Str::BooksNarrow), Choice::S(Str::BooksNormal), Choice::S(Str::BooksWide)];
const SPACINGS: &[Choice] =
    &[Choice::S(Str::BooksTight), Choice::S(Str::BooksNormal), Choice::S(Str::BooksRelaxed)];
const PALETTES: &[Choice] = &[
    Choice::S(Str::Theme),
    Choice::S(Str::ThemeLight),
    Choice::S(Str::ThemeDark),
    Choice::S(Str::BooksSepia),
];
const TURNS: &[Choice] =
    &[Choice::S(Str::Off), Choice::Secs(10), Choice::Secs(20), Choice::Secs(30), Choice::Secs(60)];
/// Seconds per page of each Auto page turn choice (0: off).
pub const TURN_SECS: [u16; 5] = [0, 10, 20, 30, 60];
const FIND_IN: &[Choice] = &[Choice::S(Str::BooksFolder), Choice::S(Str::BooksWholeCard)];

/// The reading settings in the reader menu's order, then the word.
pub const SETTINGS: [Setting; 8] = [
    c(0x0F01, Str::BooksFontSize, SIZES, 0).packed(0x0F80, 0, 2),
    c(0x0F02, Str::BooksMargins, MARGINS, 1).packed(0x0F80, 2, 2),
    c(0x0F03, Str::BooksLineSpacing, SPACINGS, 1).packed(0x0F80, 4, 2),
    t(0x0F04, Str::BooksJustify, 0).packed(0x0F80, 6, 1),
    c(0x0F05, Str::BooksPalette, PALETTES, 0).packed(0x0F80, 7, 2),
    c(0x0F06, Str::BooksAutoTurn, TURNS, 0).packed(0x0F80, 9, 3),
    c(0x0F07, Str::BooksFindIn, FIND_IN, 0).packed(0x0F80, 12, 1),
    // The word: its settings' defaults in their bits (0: every default
    // above is 0 except margins and spacing, 1 each).
    r(0x0F80, Str::AppBooks, i16::MIN, i16::MAX, 1, Unit::None, (1 << 2) | (1 << 4)),
];

/// Stored items of [`SETTINGS`] (the word).
pub const STORED_SETTINGS: usize = 1;

/// The settings the reader menu shows (all but the library's).
#[must_use]
pub fn reader_settings() -> &'static [Setting] {
    &SETTINGS[..6]
}

/// Where the library looks ("Find books in").
pub fn scope<S: SettingsService + ?Sized>(s: &S) -> Scope {
    if s.setting(id::FIND_IN) == 1 { Scope::WholeCard } else { Scope::BooksFolder }
}

/// A reading face: the font, its bold (if it has a real one), the pixel
/// scale, and its vertical metrics.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub font: &'static Font,
    pub bold: Option<&'static Font>,
    pub scale: u8,
    /// Baseline to the top of capitals and to the bottom of descenders,
    /// in screen pixels.
    pub ascent: i16,
    pub descent: i16,
    /// Line pitch for Tight, Normal, Relaxed.
    pub pitch: [u16; 3],
    /// Body paragraphs' first-line indent and quotes' inset.
    pub indent: u16,
}

/// The faces of Font size: Small (the UI font), Medium and Large (the
/// reading faces), Extra large (the UI font at twice the size: A11Y-2's
/// large text).
pub const FACES: [Face; 4] = [
    Face { font: &UI, bold: Some(&UI_BOLD), scale: 1, ascent: 9, descent: 3, pitch: [14, 16, 19], indent: 8 },
    Face { font: &BOOK16, bold: None, scale: 1, ascent: 12, descent: 4, pitch: [18, 20, 24], indent: 10 },
    Face { font: &BOOK20, bold: None, scale: 1, ascent: 15, descent: 4, pitch: [22, 25, 29], indent: 12 },
    Face {
        font: &UI,
        bold: Some(&UI_BOLD),
        scale: 2,
        ascent: 18,
        descent: 6,
        pitch: [27, 30, 35],
        indent: 16,
    },
];

/// CJK and other wide characters, drawn 16 px wide by the fallback font.
fn wide(c: char) -> bool {
    orpheus_books::layout::breaks_around(c) || matches!(u32::from(c), 0x1100..=0x115F | 0x3000..=0x303F)
}

impl Face {
    /// The face of the Font size setting.
    pub fn of<S: SettingsService + ?Sized>(s: &S) -> Face {
        FACES[usize::try_from(s.setting(id::FONT_SIZE)).unwrap_or(0).min(3)]
    }

    /// How `c` in `style` is drawn: the font, and whether it is
    /// emboldened by drawing.
    #[must_use]
    pub fn pick(&self, c: char, style: u8) -> (&'static Font, Style) {
        let mut st = Style { scale: self.scale, bold: false, slant: style & ITALIC != 0 };
        if style & BOLD != 0 {
            match self.bold {
                Some(b) if b.glyph(c).is_some() => return (b, st),
                _ => st.bold = true,
            }
        }
        (self.font, st)
    }

    /// The advance of `c` in pixels: the font's, the UI font's, or the
    /// fallback font's (16 px wide, 8 px narrow), scaled; a drawn bold is
    /// a pixel wider.
    #[must_use]
    pub fn advance(&self, c: char, style: u8) -> u16 {
        let (font, st) = self.pick(c, style);
        let base = match font.glyph(c).or_else(|| UI.glyph(c)) {
            Some(g) => u16::from(g.adv),
            None if u32::from(c) < 0x20 => 0,
            None if wide(c) => 16,
            None => 8,
        };
        base * u16::from(self.scale) + u16::from(st.bold)
    }

    /// Draw `c` with its pen at `x`, baseline `y`.
    pub fn draw(&self, g: &mut Gfx<'_>, x: i16, y: i16, c: char, style: u8, color: Color) {
        let (font, st) = self.pick(c, style);
        g.char_styled(x, y, c, font, color, st);
    }
}

impl Metrics for Face {
    fn advance(&self, c: char, style: u8) -> u16 {
        Face::advance(self, c, style)
    }
}

/// The reading area: the content area with the strip (128 × 114).
pub const AREA_W: i16 = 128;
pub const AREA_H: i16 = 114;
/// The position bar at the right edge.
pub const BAR: i16 = 3;
/// Left and right margins of Margins.
pub const MARGIN_PX: [i16; 3] = [2, 5, 9];
/// Space above the first line.
pub const TOP: i16 = 2;

/// The page geometry from the settings.
pub fn params<S: SettingsService + ?Sized>(s: &S) -> Params {
    let f = Face::of(s);
    let m = MARGIN_PX[usize::try_from(s.setting(id::MARGINS)).unwrap_or(1).min(2)];
    let sp = usize::try_from(s.setting(id::SPACING)).unwrap_or(1).min(2);
    let line = f.pitch[sp];
    Params {
        width: u16::try_from(AREA_W - 2 * m - BAR).unwrap_or(100),
        height: u16::try_from(AREA_H - TOP).unwrap_or(100),
        line,
        gap: [1u16, 3, 5][sp] * u16::from(f.scale),
        indent: f.indent,
        inset: f.indent,
        justify: s.setting(id::JUSTIFY) != 0,
        font: u16::try_from(s.setting(id::FONT_SIZE)).unwrap_or(0),
    }
}

/// The left margin in pixels.
pub fn margin<S: SettingsService + ?Sized>(s: &S) -> i16 {
    MARGIN_PX[usize::try_from(s.setting(id::MARGINS)).unwrap_or(1).min(2)]
}

/// Background and text colours of the Palette setting.
pub fn colours<S: SettingsService + ?Sized>(s: &S, g: &Gfx<'_>) -> (Color, Color) {
    match s.setting(id::PALETTE) {
        1 => (rgb(0xFF, 0xFF, 0xFF), rgb(0x1C, 0x1C, 0x1C)),
        2 => (rgb(0x00, 0x00, 0x00), rgb(0xD6, 0xD6, 0xD2)),
        3 => (rgb(0xF4, 0xEC, 0xD8), rgb(0x5B, 0x46, 0x36)),
        _ => (g.c(Role::Bg), g.c(Role::Text)),
    }
}

/// The Books app's service (part of `Services` with the feature): the
/// model in `orpheus-books` and where its requests go. The firmware runs
/// [`Shelf::step`] in its card task; the simulator between frames.
pub trait BooksService {
    fn books(&self) -> &Shelf;
    fn books_request(&mut self, r: Req);
}

/// The reader's layout for the current settings (sent when the reader
/// opens and when a setting changes).
pub fn send_params<S: SettingsService + BooksService + ?Sized>(s: &mut S) {
    let p = params(s);
    s.books_request(Req::Params(p));
}

/// A setting key of the reading settings.
#[must_use]
pub fn is_reading_setting(k: u16) -> bool {
    (0x0F01..=0x0F06).contains(&k) || k == id::WORD.0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::settings::{SettingId, SettingsCache, all, def};

    struct S(SettingsCache);
    impl SettingsService for S {
        fn setting(&self, id: SettingId) -> i16 {
            self.0.get(id)
        }
        fn set_setting(&mut self, id: SettingId, v: i16) {
            self.0.set(id, v);
        }
        fn factory_reset(&mut self) {}
    }

    /// Every reading setting is in the settings table, packed in the one
    /// word without overlaps, valid by default, and has an entry in the
    /// effects manifest (VER-13).
    #[test]
    fn settings_table() {
        let manifest = include_str!("../../../tests/settings/effects.toml");
        let mut used = 0u16;
        for s in &SETTINGS {
            assert!(def(s.id).is_some(), "{:#06x} in the table", s.id.0);
            assert_eq!(s.validate(s.default), s.default);
            if let Some((w, shift, bits)) = s.pack {
                assert_eq!(w, id::WORD);
                let mask = ((1u16 << bits) - 1) << shift;
                assert_eq!(used & mask, 0, "{:#06x} overlaps", s.id.0);
                used |= mask;
                let key = std::format!("id = \"{:#06X}\"", s.id.0).replace("0X", "0x");
                assert!(manifest.contains(&key), "{key} missing from tests/settings/effects.toml");
            }
        }
        assert_eq!(all().filter(|s| (s.id.0 >> 8) == 0x0F && s.stored()).count(), STORED_SETTINGS);
        let c = S(SettingsCache::new());
        for s in &SETTINGS[..7] {
            assert_eq!(c.setting(s.id), s.default, "{:#06x} default through the word", s.id.0);
        }
    }

    #[test]
    fn faces_and_params() {
        let mut s = S(SettingsCache::new());
        let small = params(&s);
        assert_eq!((small.width, small.line), (128 - 10 - 3, 16));
        s.set_setting(id::FONT_SIZE, 3);
        let xl = params(&s);
        assert_ne!(small.key(), xl.key(), "another face, another index");
        assert_eq!(Face::of(&s).advance('m', 0), 2 * u16::from(UI.glyph('m').unwrap().adv));
        s.set_setting(id::JUSTIFY, 1);
        assert_eq!(params(&s).key(), xl.key(), "justification does not move breaks");
        let f = FACES[1];
        assert_eq!(f.advance('W', BOLD), f.advance('W', 0) + 1, "drawn bold");
        assert_eq!(FACES[0].advance('W', BOLD), u16::from(UI_BOLD.glyph('W').unwrap().adv), "real bold");
        assert_eq!(f.advance('中', 0), 16);
        assert_eq!(scope(&s), Scope::BooksFolder);
        s.set_setting(id::FIND_IN, 1);
        assert_eq!(scope(&s), Scope::WholeCard);
    }
}
