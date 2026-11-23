//! `Gfx`: drawing over the 128 × 128 RGB565 frame buffer with a clip
//! rectangle, a local origin, palette roles, text with fallback-font lookup,
//! icon blits and dirty-rectangle tracking (05 §5.1).

use embedded_graphics_core::Pixel;
use embedded_graphics_core::draw_target::DrawTarget;
use embedded_graphics_core::geometry::{OriginDimensions, Size};
use embedded_graphics_core::pixelcolor::{IntoStorage, Rgb565};
use embedded_graphics_core::primitives::Rectangle;

use crate::color::{Color, blend};
use crate::font::{FALLBACK_RECORD, FontId, Glyph, GlyphCache, GlyphSource, UI, fallback_glyph};
use crate::geom::Rect;
use crate::icons::Icon;
use crate::text::{for_each_visual, is_cjk};
use crate::theme::{Palette, Role};

/// Panel width and height in pixels.
pub const SCREEN_W: i16 = 128;
pub const SCREEN_H: i16 = 128;
/// Pixels in the frame buffer.
pub const FB_PIXELS: usize = 128 * 128;
/// The whole panel.
pub const SCREEN: Rect = Rect::new(0, 0, SCREEN_W, SCREEN_H);
/// Status bar (05 §4).
pub const STATUS_BAR: Rect = Rect::new(0, 0, SCREEN_W, 14);
/// Content area between status bar and strip.
pub const CONTENT: Rect = Rect::new(0, 14, SCREEN_W, 96);
/// Bottom strip: mini-player, hints or toasts.
pub const STRIP: Rect = Rect::new(0, 110, SCREEN_W, 18);

/// Up to four dirty rectangles; more are merged pairwise by least added
/// area. The display task blits each one (or their union) after an event
/// batch.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirtySet {
    rects: [Rect; 4],
    n: u8,
}

impl DirtySet {
    #[must_use]
    pub const fn new() -> DirtySet {
        DirtySet { rects: [Rect::EMPTY; 4], n: 0 }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The dirty rectangles (disjoint is not guaranteed; overlap is rare).
    #[must_use]
    pub fn rects(&self) -> &[Rect] {
        self.rects.get(..usize::from(self.n)).unwrap_or(&[])
    }

    /// Union of everything dirty.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.rects().iter().fold(Rect::EMPTY, |a, r| a.union(r))
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }

    /// Return the current set and clear it.
    pub fn take(&mut self) -> DirtySet {
        let d = *self;
        self.n = 0;
        d
    }

    pub fn add(&mut self, r: Rect) {
        let r = r.intersect(&SCREEN);
        if r.is_empty() {
            return;
        }
        let mut cur = r;
        // Absorb every rectangle the new one touches, repeatedly.
        loop {
            let n = usize::from(self.n);
            let hit = self.rects[..n].iter().position(|o| o.touches(&cur));
            match hit {
                Some(i) => {
                    cur = cur.union(&self.rects[i]);
                    self.rects[i] = self.rects[n - 1];
                    self.n -= 1;
                }
                None => break,
            }
        }
        if usize::from(self.n) < self.rects.len() {
            self.rects[usize::from(self.n)] = cur;
            self.n += 1;
            return;
        }
        // Full: merge the pair (including the new one) whose union grows least.
        let mut best = (0usize, 4usize, i32::MAX);
        let all = [self.rects[0], self.rects[1], self.rects[2], self.rects[3], cur];
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                let u = all[i].union(&all[j]);
                let cost = u.area() - all[i].area() - all[j].area();
                if cost < best.2 {
                    best = (i, j, cost);
                }
            }
        }
        let (i, j, _) = best;
        let merged = all[i].union(&all[j]);
        let mut k = 0;
        for (idx, r) in all.iter().enumerate() {
            if idx != i && idx != j {
                self.rects[k] = *r;
                k += 1;
            }
        }
        self.n = 3;
        self.add(merged);
    }
}

/// The fallback font while drawing: its cache and its source.
pub struct Fallback<'a> {
    pub cache: &'a mut GlyphCache,
    pub src: &'a mut dyn GlyphSource,
}

/// Horizontal text alignment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

enum Resolved {
    Static(Glyph<'static>),
    Fallback([u8; FALLBACK_RECORD], u8),
    Missing,
    Skip,
}

/// Drawing context. Coordinates are relative to the current area (see
/// [`Gfx::with_area`]); everything is clipped to the area and the panel.
pub struct Gfx<'a> {
    px: &'a mut [u16],
    dirty: &'a mut DirtySet,
    fallback: Option<Fallback<'a>>,
    /// Colours for the palette roles.
    pub pal: Palette,
    org_x: i16,
    org_y: i16,
    area: Rect,
    clip: Rect,
}

impl<'a> Gfx<'a> {
    /// Wrap a frame buffer of at least [`FB_PIXELS`] pixels (row-major,
    /// 128 per row). A shorter buffer is clipped, never indexed out of range.
    pub fn new(
        px: &'a mut [u16],
        dirty: &'a mut DirtySet,
        pal: Palette,
        fallback: Option<Fallback<'a>>,
    ) -> Self {
        Gfx { px, dirty, fallback, pal, org_x: 0, org_y: 0, area: SCREEN, clip: SCREEN }
    }

    /// Colour of a role.
    #[must_use]
    pub fn c(&self, r: Role) -> Color {
        self.pal.get(r)
    }

    /// Width and height of the current area.
    #[must_use]
    pub fn size(&self) -> (i16, i16) {
        (self.area.w, self.area.h)
    }

    /// The current area in local coordinates.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.area.w, self.area.h)
    }

    /// Run `f` with the origin moved to `r` (local coordinates) and drawing
    /// clipped to it.
    pub fn with_area<R>(&mut self, r: Rect, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = (self.org_x, self.org_y, self.area, self.clip);
        let abs = r.translate(self.org_x, self.org_y);
        self.org_x = abs.x;
        self.org_y = abs.y;
        self.area = abs;
        self.clip = self.clip.intersect(&abs);
        let out = f(self);
        (self.org_x, self.org_y, self.area, self.clip) = saved;
        out
    }

    /// Run `f` with drawing additionally clipped to `r` (local coordinates);
    /// the origin is unchanged.
    pub fn with_clip<R>(&mut self, r: Rect, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.clip;
        self.clip = self.clip.intersect(&r.translate(self.org_x, self.org_y));
        let out = f(self);
        self.clip = saved;
        out
    }

    fn abs(&self, r: Rect) -> Rect {
        r.translate(self.org_x, self.org_y).intersect(&self.clip)
    }

    fn idx(x: i16, y: i16) -> usize {
        // Callers clip to the panel first, so both are in 0..128.
        usize::try_from(y).unwrap_or(0) * 128 + usize::try_from(x).unwrap_or(0)
    }

    /// Read a pixel (local coordinates); `None` outside the clip.
    #[must_use]
    pub fn get(&self, x: i16, y: i16) -> Option<Color> {
        let (ax, ay) = (x + self.org_x, y + self.org_y);
        if !self.clip.contains(ax, ay) {
            return None;
        }
        self.px.get(Self::idx(ax, ay)).copied()
    }

    /// Set a pixel with coverage `a` (local coordinates). Does not mark
    /// dirty; callers mark the bounding box.
    fn put_abs(&mut self, ax: i16, ay: i16, c: Color, a: u8) {
        if a == 0 || !self.clip.contains(ax, ay) {
            return;
        }
        if let Some(p) = self.px.get_mut(Self::idx(ax, ay)) {
            *p = blend(*p, c, a);
        }
    }

    /// Set one pixel.
    pub fn pixel(&mut self, x: i16, y: i16, c: Color) {
        let (ax, ay) = (x + self.org_x, y + self.org_y);
        self.put_abs(ax, ay, c, 255);
        self.dirty.add(Rect::new(ax, ay, 1, 1).intersect(&self.clip));
    }

    /// Mark a local rectangle dirty without drawing (e.g. after a direct
    /// frame-buffer write by an image decoder).
    pub fn mark(&mut self, r: Rect) {
        let a = self.abs(r);
        self.dirty.add(a);
    }

    /// Fill a rectangle.
    pub fn fill(&mut self, r: Rect, c: Color) {
        let a = self.abs(r);
        if a.is_empty() {
            return;
        }
        for y in a.y..a.bottom() {
            let s = Self::idx(a.x, y);
            if let Some(row) = self.px.get_mut(s..s + usize::try_from(a.w).unwrap_or(0)) {
                row.fill(c);
            }
        }
        self.dirty.add(a);
    }

    /// Fill with a role colour.
    pub fn fill_role(&mut self, r: Rect, role: Role) {
        self.fill(r, self.c(role));
    }

    /// Clear the whole current area to a role.
    pub fn clear(&mut self, role: Role) {
        self.fill_role(self.bounds(), role);
    }

    pub fn hline(&mut self, x: i16, y: i16, w: i16, c: Color) {
        self.fill(Rect::new(x, y, w, 1), c);
    }

    pub fn vline(&mut self, x: i16, y: i16, h: i16, c: Color) {
        self.fill(Rect::new(x, y, 1, h), c);
    }

    /// One-pixel rectangle outline.
    pub fn stroke(&mut self, r: Rect, c: Color) {
        self.hline(r.x, r.y, r.w, c);
        self.hline(r.x, r.bottom() - 1, r.w, c);
        self.vline(r.x, r.y + 1, r.h - 2, c);
        self.vline(r.right() - 1, r.y + 1, r.h - 2, c);
    }

    /// Blend every pixel of `r` towards `c` by `a`/255 (scrims behind
    /// sheets and dialogs).
    pub fn dim(&mut self, r: Rect, c: Color, a: u8) {
        let ar = self.abs(r);
        for y in ar.y..ar.bottom() {
            for x in ar.x..ar.right() {
                self.put_abs(x, y, c, a);
            }
        }
        self.dirty.add(ar);
    }

    /// Coverage (0..=16) of pixel (`px`, `py`) by a disc of radius `r`
    /// centred at (`cx`, `cy`) (all in pixel-edge units), by 4 × 4
    /// supersampling.
    fn disc_cov(px: i16, py: i16, cx: i16, cy: i16, r: i16) -> u8 {
        let rr = i32::from(r) * 8 * i32::from(r) * 8;
        let mut n = 0u8;
        for j in 0..4 {
            let sy = i32::from(py) * 8 + 2 * j + 1 - i32::from(cy) * 8;
            for i in 0..4 {
                let sx = i32::from(px) * 8 + 2 * i + 1 - i32::from(cx) * 8;
                if sx * sx + sy * sy <= rr {
                    n += 1;
                }
            }
        }
        n
    }

    /// Coverage of a pixel in rectangle-local coordinates by a rounded
    /// rectangle of size `w × h` and radius `r`.
    fn round_cov(x: i16, y: i16, w: i16, h: i16, r: i16) -> u8 {
        let cx = if x < r {
            r
        } else if x >= w - r {
            w - r
        } else {
            return 16;
        };
        let cy = if y < r {
            r
        } else if y >= h - r {
            h - r
        } else {
            return 16;
        };
        Self::disc_cov(x, y, cx, cy, r)
    }

    fn cov_alpha(n: u8) -> u8 {
        if n >= 16 { 255 } else { n * 16 }
    }

    /// Filled rectangle with anti-aliased corners of radius `r`.
    pub fn fill_round(&mut self, rect: Rect, r: i16, c: Color) {
        let r = r.clamp(0, rect.w.min(rect.h) / 2);
        if r == 0 {
            self.fill(rect, c);
            return;
        }
        // Straight middle band in one go.
        self.fill(Rect::new(rect.x, rect.y + r, rect.w, rect.h - 2 * r), c);
        for yy in (0..r).chain(rect.h - r..rect.h) {
            self.fill(Rect::new(rect.x + r, rect.y + yy, rect.w - 2 * r, 1), c);
            for xx in (0..r).chain(rect.w - r..rect.w) {
                let n = Self::round_cov(xx, yy, rect.w, rect.h, r);
                let (ax, ay) = (rect.x + xx + self.org_x, rect.y + yy + self.org_y);
                self.put_abs(ax, ay, c, Self::cov_alpha(n));
            }
        }
        let a = self.abs(rect);
        self.dirty.add(a);
    }

    /// One-pixel anti-aliased outline of a rounded rectangle.
    pub fn stroke_round(&mut self, rect: Rect, r: i16, c: Color) {
        let r = r.clamp(1, rect.w.min(rect.h) / 2);
        self.hline(rect.x + r, rect.y, rect.w - 2 * r, c);
        self.hline(rect.x + r, rect.bottom() - 1, rect.w - 2 * r, c);
        self.vline(rect.x, rect.y + r, rect.h - 2 * r, c);
        self.vline(rect.right() - 1, rect.y + r, rect.h - 2 * r, c);
        for yy in (0..r).chain(rect.h - r..rect.h) {
            for xx in (0..r).chain(rect.w - r..rect.w) {
                let outer = Self::round_cov(xx, yy, rect.w, rect.h, r);
                let inner = Self::round_cov(xx - 1, yy - 1, rect.w - 2, rect.h - 2, r - 1);
                let inner =
                    if xx == 0 || yy == 0 || xx == rect.w - 1 || yy == rect.h - 1 { 0 } else { inner };
                let n = outer.saturating_sub(inner);
                let (ax, ay) = (rect.x + xx + self.org_x, rect.y + yy + self.org_y);
                self.put_abs(ax, ay, c, Self::cov_alpha(n));
            }
        }
        let a = self.abs(rect);
        self.dirty.add(a);
    }

    /// Filled anti-aliased disc inscribed in `rect` (use even sizes).
    pub fn fill_disc(&mut self, rect: Rect, c: Color) {
        self.fill_round(rect, rect.w.min(rect.h) / 2, c);
    }

    /// Draw a 1-bit icon tinted `c`, top-left at (`x`, `y`).
    pub fn icon(&mut self, x: i16, y: i16, icon: &Icon, c: Color) {
        let (ox, oy) = (x + self.org_x, y + self.org_y);
        for (row, bits) in icon.rows.iter().enumerate() {
            let ay = oy + i16::try_from(row).unwrap_or(0);
            if *bits == 0 {
                continue;
            }
            for col in 0..i16::from(icon.w) {
                if bits & (0x8000 >> col) != 0 {
                    self.put_abs(ox + col, ay, c, 255);
                }
            }
        }
        let a = self.abs(Rect::new(x, y, i16::from(icon.w), i16::from(icon.h)));
        self.dirty.add(a);
    }

    /// Blit an RGB565 image of `w` pixels per row.
    pub fn image(&mut self, x: i16, y: i16, w: i16, data: &[u16]) {
        let wu = usize::try_from(w).unwrap_or(0);
        if wu == 0 {
            return;
        }
        let h = i16::try_from(data.len() / wu).unwrap_or(0);
        let a = self.abs(Rect::new(x, y, w, h));
        for ay in a.y..a.bottom() {
            let sy = usize::try_from(ay - y - self.org_y).unwrap_or(0);
            let sx = usize::try_from(a.x - x - self.org_x).unwrap_or(0);
            let src = data.get(sy * wu + sx..sy * wu + sx + usize::try_from(a.w).unwrap_or(0));
            let d = Self::idx(a.x, ay);
            if let (Some(src), Some(dst)) = (src, self.px.get_mut(d..d + usize::try_from(a.w).unwrap_or(0))) {
                dst.copy_from_slice(src);
            }
        }
        self.dirty.add(a);
    }

    // ----- text --------------------------------------------------------

    fn resolve(&mut self, ch: char, font: FontId) -> Resolved {
        if u32::from(ch) < 0x20 || ch == '\u{200b}' || ch == '\u{feff}' {
            return Resolved::Skip;
        }
        if let Some(g) = font.font().glyph(ch) {
            return Resolved::Static(g);
        }
        if font != FontId::Ui
            && let Some(g) = UI.glyph(ch)
        {
            return Resolved::Static(g);
        }
        let Ok(cp) = u16::try_from(u32::from(ch)) else {
            return Resolved::Missing;
        };
        if let Some(fb) = self.fallback.as_mut() {
            let base = fb.src.baseline();
            if let Some(rec) = fb.cache.get(fb.src, cp)
                && fallback_glyph(rec, base).is_some()
            {
                return Resolved::Fallback(*rec, base);
            }
        }
        Resolved::Missing
    }

    fn advance_of(&mut self, ch: char, font: FontId) -> i16 {
        match self.resolve(ch, font) {
            Resolved::Static(g) => i16::from(g.adv),
            Resolved::Fallback(rec, b) => fallback_glyph(&rec, b).map_or(0, |g| i16::from(g.adv)),
            Resolved::Missing => 8,
            Resolved::Skip => 0,
        }
    }

    /// True if any character of `s` comes from the 16 px fallback font;
    /// such text needs a 16 px line pitch.
    #[must_use]
    pub fn needs_fallback(&self, s: &str, font: FontId) -> bool {
        s.chars().any(|c| u32::from(c) >= 0x20 && font.font().glyph(c).is_none() && UI.glyph(c).is_none())
    }

    /// Line pitch for wrapped text in `font`: 14 px, or 16 px if the text
    /// uses the fallback font.
    #[must_use]
    pub fn line_pitch(&self, s: &str, font: FontId) -> i16 {
        if self.needs_fallback(s, font) { 16 } else { 14 }
    }

    /// Width of `s` in pixels.
    pub fn measure(&mut self, s: &str, font: FontId) -> i16 {
        let mut w = 0i16;
        for ch in s.chars() {
            w = w.saturating_add(self.advance_of(ch, font));
        }
        w
    }

    fn blit_glyph(&mut self, ax: i16, ay_base: i16, g: &Glyph<'_>, c: Color) -> Rect {
        let x0 = ax + i16::from(g.x);
        let y0 = ay_base - i16::from(g.top);
        let (w, h) = (i16::from(g.w), i16::from(g.h));
        let r = Rect::new(x0, y0, w, h).intersect(&self.clip);
        if r.is_empty() {
            return Rect::EMPTY;
        }
        let bpp = usize::from(g.bpp.clamp(1, 8));
        let maxv = (1u16 << bpp) - 1;
        let wu = usize::from(g.w);
        for y in r.y..r.bottom() {
            let row = usize::try_from(y - y0).unwrap_or(0);
            for x in r.x..r.right() {
                let col = usize::try_from(x - x0).unwrap_or(0);
                let bit = (row * wu + col) * bpp;
                let Some(&byte) = g.bits.get(bit >> 3) else { continue };
                let shift = 8 - bpp - (bit & 7);
                let lv = (u16::from(byte) >> shift) & maxv;
                if lv == 0 {
                    continue;
                }
                let a = if lv == maxv { 255 } else { u8::try_from(lv * 255 / maxv).unwrap_or(255) };
                self.put_abs(x, y, c, a);
            }
        }
        r
    }

    fn draw_char(&mut self, ax: i16, ay: i16, ch: char, font: FontId, c: Color) -> (i16, Rect) {
        match self.resolve(ch, font) {
            Resolved::Static(g) => (i16::from(g.adv), self.blit_glyph(ax, ay, &g, c)),
            Resolved::Fallback(rec, b) => match fallback_glyph(&rec, b) {
                Some(g) => (i16::from(g.adv), self.blit_glyph(ax, ay, &g, c)),
                None => (0, Rect::EMPTY),
            },
            Resolved::Missing => {
                // Tofu: a hollow box the height of a capital.
                let r = Rect::new(ax + 1, ay - 9, 6, 9);
                let r2 = r.intersect(&self.clip);
                for x in r2.x..r2.right() {
                    for y in r2.y..r2.bottom() {
                        if x == r.x || x == r.right() - 1 || y == r.y || y == r.bottom() - 1 {
                            self.put_abs(x, y, c, 160);
                        }
                    }
                }
                (8, r2)
            }
            Resolved::Skip => (0, Rect::EMPTY),
        }
    }

    /// Draw `s` with its baseline at `y`, starting at `x`. Right-to-left runs
    /// are reversed (no shaping). Returns the x after the last glyph.
    pub fn text(&mut self, x: i16, y: i16, s: &str, font: FontId, c: Color) -> i16 {
        let (mut ax, ay) = (x + self.org_x, y + self.org_y);
        let right = self.clip.right();
        let mut drawn = Rect::EMPTY;
        for_each_visual(s, |ch| {
            if ax >= right {
                return false;
            }
            let (adv, r) = self.draw_char(ax, ay, ch, font, c);
            drawn = drawn.union(&r);
            ax += adv;
            true
        });
        self.dirty.add(drawn);
        ax - self.org_x
    }

    /// Draw `s` aligned inside `[x, x + w)`; if it does not fit it is cut
    /// with an ellipsis. Returns the drawn width.
    #[allow(clippy::too_many_arguments)]
    pub fn text_in(&mut self, x: i16, y: i16, w: i16, s: &str, font: FontId, c: Color, align: Align) -> i16 {
        let full = self.measure(s, font);
        if full <= w {
            let x0 = match align {
                Align::Left => x,
                Align::Center => x + (w - full) / 2,
                Align::Right => x + w - full,
            };
            self.text(x0, y, s, font, c);
            return full;
        }
        let ell = self.advance_of('…', font);
        let budget = w - ell;
        let (mut ax, ay) = (x + self.org_x, y + self.org_y);
        let mut drawn = Rect::EMPTY;
        let mut used = 0;
        for_each_visual(s, |ch| {
            let adv = self.advance_of(ch, font);
            if used + adv > budget {
                return false;
            }
            let (_, r) = self.draw_char(ax, ay, ch, font, c);
            drawn = drawn.union(&r);
            ax += adv;
            used += adv;
            true
        });
        let (_, r) = self.draw_char(ax, ay, '…', font, c);
        self.dirty.add(drawn.union(&r));
        used + ell
    }

    /// Lay `s` out in lines no wider than `w`, breaking at spaces and around
    /// CJK characters; calls `f(line)` for each. Returns the number of lines.
    pub fn wrap<'s>(
        &mut self,
        s: &'s str,
        font: FontId,
        w: i16,
        mut f: impl FnMut(&mut Self, &'s str),
    ) -> usize {
        let mut lines = 0;
        let mut rest = s.trim_start();
        while !rest.is_empty() {
            let mut width = 0i16;
            let mut brk: Option<usize> = None; // end of the line at the last break chance
            let mut end = rest.len();
            let mut prev_cjk = false;
            for (i, ch) in rest.char_indices() {
                if ch == '\n' {
                    end = i;
                    break;
                }
                let cjk = is_cjk(ch);
                if ch == ' ' || cjk || prev_cjk {
                    brk = Some(i);
                }
                prev_cjk = cjk;
                width += self.advance_of(ch, font);
                if width > w && i > 0 {
                    end = match brk {
                        Some(b) if b > 0 => b,
                        _ => i,
                    };
                    break;
                }
            }
            let (line, tail) = rest.split_at(end);
            f(self, line.trim_end());
            lines += 1;
            rest = tail.strip_prefix('\n').unwrap_or(tail).trim_start_matches(' ');
        }
        lines
    }
}

impl OriginDimensions for Gfx<'_> {
    fn size(&self) -> Size {
        Size::new(u32::try_from(self.area.w).unwrap_or(0), u32::try_from(self.area.h).unwrap_or(0))
    }
}

/// `embedded-graphics` interop: any e-g primitive or image can be drawn
/// into the current area.
impl DrawTarget for Gfx<'_> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let mut drawn = Rect::EMPTY;
        for Pixel(p, c) in pixels {
            let (Ok(x), Ok(y)) = (i16::try_from(p.x), i16::try_from(p.y)) else { continue };
            let (ax, ay) = (x + self.org_x, y + self.org_y);
            if self.clip.contains(ax, ay) {
                self.put_abs(ax, ay, c.into_storage(), 255);
                drawn = drawn.union(&Rect::new(ax, ay, 1, 1));
            }
        }
        self.dirty.add(drawn);
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let conv = |v: i32| i16::try_from(v.clamp(-1000, 1000)).unwrap_or(0);
        let w = conv(i32::try_from(area.size.width).unwrap_or(1000));
        let h = conv(i32::try_from(area.size.height).unwrap_or(1000));
        self.fill(Rect::new(conv(area.top_left.x), conv(area.top_left.y), w, h), color.into_storage());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::theme::{Accent, Theme};
    use std::vec;

    #[test]
    fn dirty_merging() {
        let mut d = DirtySet::new();
        d.add(Rect::new(0, 0, 10, 10));
        d.add(Rect::new(5, 5, 10, 10));
        assert_eq!(d.rects().len(), 1);
        d.add(Rect::new(50, 0, 4, 4));
        d.add(Rect::new(0, 100, 4, 4));
        d.add(Rect::new(100, 100, 4, 4));
        assert_eq!(d.rects().len(), 4);
        d.add(Rect::new(100, 50, 4, 4));
        assert!(d.rects().len() <= 4);
        let b = d.bounds();
        assert!(b.contains_rect(&Rect::new(100, 50, 4, 4)));
        assert!(b.contains_rect(&Rect::new(0, 0, 15, 15)));
        d.add(Rect::new(200, 200, 5, 5));
        assert!(!d.bounds().contains(200, 200));
    }

    #[test]
    fn clipping_and_dirty() {
        let mut fb = vec![0u16; FB_PIXELS];
        let mut d = DirtySet::new();
        let pal = Theme::Dark.palette(Accent::Blue);
        {
            let mut g = Gfx::new(&mut fb, &mut d, pal, None);
            g.with_area(Rect::new(10, 10, 20, 20), |g| {
                g.fill(Rect::new(-5, -5, 100, 100), 0xFFFF);
            });
        }
        assert_eq!(d.bounds(), Rect::new(10, 10, 20, 20));
        assert_eq!(fb[9 * 128 + 9], 0);
        assert_eq!(fb[10 * 128 + 10], 0xFFFF);
        assert_eq!(fb[29 * 128 + 29], 0xFFFF);
        assert_eq!(fb[30 * 128 + 30], 0);
    }

    #[test]
    fn embedded_graphics_interop() {
        use embedded_graphics_core::geometry::Point;
        use embedded_graphics_core::pixelcolor::RgbColor;
        let mut fb = vec![0u16; FB_PIXELS];
        let mut d = DirtySet::new();
        let pal = Theme::Dark.palette(Accent::Blue);
        {
            let mut g = Gfx::new(&mut fb, &mut d, pal, None);
            g.with_area(Rect::new(10, 20, 30, 30), |g| {
                assert_eq!(g.size(), (30, 30));
                g.fill_solid(&Rectangle::new(Point::new(1, 1), Size::new(2, 2)), Rgb565::RED).unwrap();
                g.draw_iter([
                    Pixel(Point::new(29, 29), Rgb565::GREEN),
                    Pixel(Point::new(40, 0), Rgb565::BLUE),
                ])
                .unwrap();
            });
        }
        assert_eq!(fb[21 * 128 + 11], 0xF800);
        assert_eq!(fb[22 * 128 + 12], 0xF800);
        assert_eq!(fb[49 * 128 + 39], 0x07E0);
        assert!(!fb.contains(&0x001F), "clipped");
        assert_eq!(d.bounds(), Rect::new(11, 21, 29, 29));
    }

    #[test]
    fn text_measure_and_wrap() {
        let mut fb = vec![0u16; FB_PIXELS];
        let mut d = DirtySet::new();
        let pal = Theme::Dark.palette(Accent::Blue);
        let mut g = Gfx::new(&mut fb, &mut d, pal, None);
        let w = g.measure("Hello", FontId::Ui);
        assert!(w > 20 && w < 40, "{w}");
        let end = g.text(0, 12, "Hello", FontId::Ui, 0xFFFF);
        assert_eq!(end, w);
        let mut lines = std::vec::Vec::new();
        let n = g.wrap("The quick brown fox jumps over the lazy dog", FontId::Ui, 60, |_, l| lines.push(l));
        assert_eq!(n, lines.len());
        assert!(n >= 3);
        for l in &lines {
            assert!(g.measure(l, FontId::Ui) <= 60, "{l}");
        }
        let used = g.text_in(0, 30, 30, "A very long line of text", FontId::Ui, 0xFFFF, Align::Left);
        assert!(used <= 30);
    }
}
