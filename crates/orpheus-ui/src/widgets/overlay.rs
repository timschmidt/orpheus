//! Transient overlays that never take focus: `Toast` (05 §5.2, 2 s over the
//! strip) and `VolumeOverlay` (1.2 s, over anything).

use crate::event::Instant;
use crate::font::FontId;
use crate::geom::Rect;
use crate::gfx::{Align, Gfx, SCREEN_H, SCREEN_W, STRIP};
use crate::icons;
use crate::text::StrBuf;
use crate::theme::Role;

pub const TOAST_MS: u64 = 2000;
pub const VOLUME_MS: u64 = 1200;

/// A one-line message over the strip.
#[derive(Clone, Copy, Debug)]
pub struct Toast {
    pub text: StrBuf<48>,
    pub until: Instant,
}

impl Toast {
    #[must_use]
    pub fn new(text: StrBuf<48>, now: Instant) -> Toast {
        Toast { text, until: now.add_ms(TOAST_MS) }
    }

    /// Area the toast covers (panel coordinates).
    #[must_use]
    pub const fn rect() -> Rect {
        Rect::new(2, STRIP.y + 1, SCREEN_W - 4, 16)
    }

    /// Draw (panel coordinates): an inverted pill so it reads as transient.
    pub fn draw(&self, g: &mut Gfx<'_>) {
        let r = Toast::rect();
        let bg = g.c(Role::Bg);
        g.fill(Rect::new(r.x - 2, r.y - 1, r.w + 4, r.h + 2), bg);
        let pill = g.c(Role::Text);
        g.fill_round(r, 8, pill);
        g.text_in(r.x + 7, r.y + 12, r.w - 14, self.text.as_str(), FontId::Ui, bg, Align::Center);
    }
}

/// Large volume number and bar.
#[derive(Clone, Copy, Debug)]
pub struct VolumeOverlay {
    pub level: u8,
    pub max: u8,
    pub until: Instant,
}

impl VolumeOverlay {
    #[must_use]
    pub const fn rect() -> Rect {
        Rect::new((SCREEN_W - 80) / 2, (SCREEN_H - 62) / 2 + 4, 80, 62)
    }

    pub fn draw(&self, g: &mut Gfx<'_>) {
        let r = VolumeOverlay::rect();
        let surf = g.c(Role::Surface);
        g.fill_round(r, 10, surf);
        let mut b = StrBuf::<4>::new();
        b.push_u32(u32::from(self.level));
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // Speaker and number centred as one group.
        let nw = g.measure(b.as_str(), FontId::Num28);
        let gw = i16::from(icons::SPEAKER.w) + 5 + nw;
        let x = r.x + (r.w - gw) / 2;
        g.icon(x, r.y + 18, &icons::SPEAKER, dim);
        g.text(x + i16::from(icons::SPEAKER.w) + 5, r.y + 38, b.as_str(), FontId::Num28, text);
        let bar = Rect::new(r.x + 10, r.y + 48, r.w - 20, 5);
        let track = g.pal.mix(Role::Surface, Role::TextDim, 80);
        g.fill_round(bar, 2, track);
        let max = i32::from(self.max.max(1));
        let fw = i16::try_from(i32::from(bar.w) * i32::from(self.level.min(self.max)) / max).unwrap_or(0);
        if fw > 0 {
            let acc = if self.level >= self.max { g.c(Role::Warning) } else { g.c(Role::Accent) };
            g.fill_round(Rect::new(bar.x, bar.y, fw.max(5), bar.h), 2, acc);
        }
    }
}
