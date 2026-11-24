//! Themes: 8-entry palettes addressed by role (05 §4 "Colour").

use crate::color::{Color, blend, rgb};

/// What a colour is for. All drawing goes through a role.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Role {
    /// Screen background.
    Bg = 0,
    /// Bars, cards, sheets.
    Surface,
    /// Primary text and icons.
    Text,
    /// Secondary text (artist, values, hints).
    TextDim,
    /// Focus and selection.
    Accent,
    /// Text and icons drawn on the accent colour.
    OnAccent,
    /// Errors, destructive actions, battery below 10 %.
    Warning,
    /// Success, charging.
    Good,
}

/// A resolved palette, one colour per [`Role`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette(pub [Color; 8]);

impl Palette {
    #[must_use]
    pub const fn get(&self, r: Role) -> Color {
        self.0[r as usize]
    }

    /// `b` over `a` at `alpha`/255: hairlines, tracks and placeholders are
    /// mixes of two roles, never new literals.
    #[must_use]
    pub fn mix(&self, a: Role, b: Role, alpha: u8) -> Color {
        blend(self.get(a), self.get(b), alpha)
    }

    #[must_use]
    pub fn is_dark(&self) -> bool {
        let (r, g, b) = crate::color::channels(self.get(Role::Bg));
        u32::from(r) * 2 + u32::from(g) + u32::from(b) * 2 < 96
    }
}

/// Base theme (Settings → Display → Theme).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    /// High contrast black and amber; ignores the accent setting.
    Amber,
}

/// Accent variant (Settings → Display → Accent).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Accent {
    #[default]
    Blue,
    Green,
    Pink,
    Red,
    Grey,
}

impl Accent {
    pub const ALL: [Accent; 5] = [Accent::Blue, Accent::Green, Accent::Pink, Accent::Red, Accent::Grey];

    /// (on dark backgrounds, on light backgrounds). Both carry white text at
    /// 4.5:1 or better (checked by a unit test).
    const fn colors(self) -> (Color, Color) {
        match self {
            Accent::Blue => (rgb(32, 110, 220), rgb(0, 92, 200)),
            Accent::Green => (rgb(22, 126, 74), rgb(16, 116, 64)),
            Accent::Pink => (rgb(200, 40, 120), rgb(184, 28, 108)),
            Accent::Red => (rgb(204, 44, 44), rgb(186, 30, 30)),
            Accent::Grey => (rgb(88, 96, 110), rgb(84, 92, 106)),
        }
    }
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Dark, Theme::Light, Theme::Amber];

    #[must_use]
    pub const fn palette(self, accent: Accent) -> Palette {
        let (on_dark, on_light) = accent.colors();
        match self {
            Theme::Dark => Palette([
                rgb(14, 16, 20),    // Bg
                rgb(36, 40, 48),    // Surface
                rgb(238, 240, 244), // Text
                rgb(150, 156, 168), // TextDim
                on_dark,            // Accent
                rgb(255, 255, 255), // OnAccent
                rgb(255, 96, 88),   // Warning
                rgb(72, 204, 120),  // Good
            ]),
            Theme::Light => Palette([
                rgb(248, 248, 250),
                rgb(226, 229, 235),
                rgb(18, 20, 24),
                rgb(88, 94, 104),
                on_light,
                rgb(255, 255, 255),
                rgb(196, 32, 32),
                rgb(16, 128, 64),
            ]),
            Theme::Amber => Palette([
                rgb(0, 0, 0),
                rgb(40, 26, 0),
                rgb(255, 184, 0),
                rgb(208, 140, 0),
                rgb(255, 184, 0),
                rgb(0, 0, 0),
                rgb(255, 72, 40),
                rgb(255, 224, 120),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn lum(c: Color) -> f32 {
        let [r, g, b] = crate::color::to_rgb888(c);
        let ch = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.039_28 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 05 §2.7: 4.5:1 minimum for every text/background pairing we draw.
    #[test]
    fn contrast_minimum() {
        for t in Theme::ALL {
            for a in Accent::ALL {
                let p = t.palette(a);
                let pairs = [
                    (Role::Text, Role::Bg),
                    (Role::Text, Role::Surface),
                    (Role::TextDim, Role::Bg),
                    (Role::TextDim, Role::Surface),
                    (Role::OnAccent, Role::Accent),
                    (Role::Warning, Role::Bg),
                ];
                for (f, b) in pairs {
                    let c = contrast(p.get(f), p.get(b));
                    assert!(c >= 4.5, "{t:?}/{a:?}: {f:?} on {b:?} is {c:.2}:1");
                }
            }
        }
    }
}
