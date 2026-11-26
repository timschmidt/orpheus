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
    /// High contrast, dark (A11Y-2, 05 §7.14): white on black, a yellow
    /// accent with black text; every text pairing at 7:1 or better (WCAG
    /// AAA), checked by a unit test. Ignores the accent setting.
    ContrastDark,
    /// High contrast, light: black on white, a dark blue accent with white
    /// text; 7:1 or better. Ignores the accent setting.
    ContrastLight,
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
    pub const ALL: [Theme; 5] =
        [Theme::Dark, Theme::Light, Theme::Amber, Theme::ContrastDark, Theme::ContrastLight];

    /// From Settings › Display › Theme's value (anything else: Dark).
    #[must_use]
    pub const fn from_setting(v: i16) -> Theme {
        match v {
            1 => Theme::Light,
            2 => Theme::Amber,
            3 => Theme::ContrastDark,
            4 => Theme::ContrastLight,
            _ => Theme::Dark,
        }
    }

    /// The high-contrast themes (A11Y-2): 7:1 for every text pairing.
    #[must_use]
    pub const fn high_contrast(self) -> bool {
        matches!(self, Theme::ContrastDark | Theme::ContrastLight)
    }

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
            Theme::ContrastDark => Palette([
                rgb(0, 0, 0),
                rgb(30, 30, 30),
                rgb(255, 255, 255),
                rgb(208, 208, 208),
                rgb(255, 220, 0),
                rgb(0, 0, 0),
                rgb(255, 176, 160),
                rgb(80, 240, 120),
            ]),
            Theme::ContrastLight => Palette([
                rgb(255, 255, 255),
                rgb(240, 240, 240),
                rgb(0, 0, 0),
                rgb(56, 56, 56),
                rgb(0, 44, 144),
                rgb(255, 255, 255),
                rgb(120, 0, 0),
                rgb(0, 88, 0),
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

    /// Every colour pairing the toolkit draws text in, as drawn (RGB565,
    /// blends included): (name, foreground, background).
    fn text_pairs(p: &Palette) -> std::vec::Vec<(&'static str, Color, Color)> {
        use crate::color::blend;
        let g = |r| p.get(r);
        let button = p.mix(Role::Surface, Role::Text, 28);
        std::vec![
            ("text on bg", g(Role::Text), g(Role::Bg)),
            ("text on surface", g(Role::Text), g(Role::Surface)),
            ("dim on bg", g(Role::TextDim), g(Role::Bg)),
            ("dim on surface", g(Role::TextDim), g(Role::Surface)),
            ("on-accent on accent", g(Role::OnAccent), g(Role::Accent)),
            ("warning on bg", g(Role::Warning), g(Role::Bg)),
            ("warning on surface", g(Role::Warning), g(Role::Surface)),
            ("good on bg", g(Role::Good), g(Role::Bg)),
            // A focused row's value (list.rs).
            ("focused value", p.mix(Role::Accent, Role::OnAccent, 200), g(Role::Accent)),
            // A list's section header (list.rs: lifted on dark, plain on
            // light backgrounds).
            (
                "header on bg",
                if p.is_dark() { blend(g(Role::Accent), g(Role::Text), 110) } else { g(Role::Accent) },
                g(Role::Bg),
            ),
            ("header on surface", blend(g(Role::Accent), g(Role::Text), 110), g(Role::Surface)),
            // A dialog's unfocused buttons (dialog.rs).
            ("button label", g(Role::Text), button),
            ("danger button label", g(Role::Warning), button),
        ]
    }

    /// A11Y-2 (05 §7.14 "high-contrast themes"): every text pairing of
    /// the two high-contrast themes at 7:1 or better (WCAG 2 AAA for body
    /// text), with every accent setting (they ignore it).
    #[test]
    fn high_contrast_themes_reach_7_to_1() {
        let mut worst = f32::MAX;
        for t in Theme::ALL.into_iter().filter(|t| t.high_contrast()) {
            for a in Accent::ALL {
                let p = t.palette(a);
                assert_eq!(p, t.palette(Accent::Blue), "{t:?} ignores the accent");
                for (name, f, b) in text_pairs(&p) {
                    let c = contrast(f, b);
                    worst = worst.min(c);
                    assert!(c >= 7.0, "{t:?}: {name} is {c:.2}:1");
                }
            }
        }
        assert!(worst >= 7.0, "{worst}");
    }

    /// The ratios of every theme's text pairings, for the record (`cargo
    /// test -p orpheus-ui contrast_table -- --nocapture`); the base pairs
    /// are held to 4.5:1 by `contrast_minimum`.
    #[test]
    fn contrast_table() {
        for t in Theme::ALL {
            let p = t.palette(Accent::Blue);
            let mut line = std::format!("{t:?}:");
            let mut low = f32::MAX;
            for (name, f, b) in text_pairs(&p) {
                let c = contrast(f, b);
                low = low.min(c);
                line.push_str(&std::format!(" {name} {c:.1};"));
            }
            std::println!("{line} lowest {low:.2}");
            assert!(low >= 3.0, "{t:?}: {line}");
        }
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
