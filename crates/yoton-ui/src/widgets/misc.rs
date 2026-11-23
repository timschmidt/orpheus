//! Small stateless widgets: `Progress`, `Spinner`, `BigValue` (05 §5.2).

use crate::event::Instant;
use crate::font::FontId;
use crate::geom::Rect;
use crate::gfx::{Align, Gfx};
use crate::text::StrBuf;
use crate::theme::Role;

/// Horizontal progress bar with an optional buffered range (0..=1000 each).
pub fn draw_progress(g: &mut Gfx<'_>, r: Rect, value: u16, buffered: Option<u16>, bg: Role) {
    let rad = r.h / 2;
    let track = g.pal.mix(bg, Role::TextDim, 70);
    g.fill_role(r, bg);
    g.fill_round(r, rad, track);
    let wpx = |v: u16| i16::try_from(i32::from(r.w) * i32::from(v.min(1000)) / 1000).unwrap_or(0);
    if let Some(b) = buffered {
        let c = g.pal.mix(bg, Role::TextDim, 140);
        g.fill_round(Rect::new(r.x, r.y, wpx(b).max(r.h), r.h), rad, c);
    }
    let w = wpx(value);
    if w > 0 {
        let c = g.c(Role::Accent);
        g.fill_round(Rect::new(r.x, r.y, w.max(r.h), r.h), rad, c);
    }
}

/// Indeterminate spinner: eight dots around a circle in a 14 × 14 box, the
/// head chosen by time (8 steps per second).
pub fn draw_spinner(g: &mut Gfx<'_>, x: i16, y: i16, now: Instant, bg: Role) {
    const DOTS: [(i16, i16); 8] = [(5, 0), (9, 1), (10, 5), (9, 9), (5, 10), (1, 9), (0, 5), (1, 1)];
    g.fill_role(Rect::new(x, y, 14, 14), bg);
    let head = usize::try_from((now.0 / 125) % 8).unwrap_or(0);
    for (i, (dx, dy)) in DOTS.iter().enumerate() {
        let age = (head + 8 - i) % 8;
        let a = 255u8.saturating_sub(u8::try_from(age * 30).unwrap_or(255));
        let c = g.pal.mix(bg, Role::Text, a.max(40));
        g.fill_disc(Rect::new(x + dx, y + dy, 4, 4), c);
    }
}

/// Large-numeral value with unit and caption (clock, frequency, steps).
/// `value` uses the numeral font; the unit is drawn after it in the UI
/// font; the caption is centred below. Everything is centred in `r`.
///
/// If value and unit do not fit side by side, the unit moves to the start
/// of the caption line.
pub fn draw_big_value(g: &mut Gfx<'_>, r: Rect, value: &str, unit: &str, caption: &str, bg: Role) {
    g.fill_role(r, bg);
    let vw = g.measure(value, FontId::Num28);
    let uw = if unit.is_empty() { 0 } else { g.measure(unit, FontId::Ui) + 3 };
    let unit_inline = vw + uw <= r.w;
    let total = if unit_inline { vw + uw } else { vw };
    let x = r.x + (r.w - total) / 2;
    let base = r.y + 30;
    let text = g.c(Role::Text);
    let dim = g.c(Role::TextDim);
    let end = g.text(x, base, value, FontId::Num28, text);
    if unit_inline && !unit.is_empty() {
        g.text(end + 3, base, unit, FontId::Ui, dim);
    }
    let mut line = StrBuf::<64>::new();
    if !unit_inline && !unit.is_empty() {
        line.push_str(unit);
        if !caption.is_empty() {
            line.push_str(" · ");
        }
    }
    line.push_str(caption);
    if !line.is_empty() {
        g.text_in(r.x, base + 16, r.w, line.as_str(), FontId::Ui, dim, Align::Center);
    }
}
