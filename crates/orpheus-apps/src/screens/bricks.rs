//! Bricks on 128 × 128 (05 §7.13, GAME-6): drawing.
//!
//! Full screen: a status line (score, level, lives as dots) over the
//! 128 × 116 field. Bricks 9 × 4 px in their 10 × 5 cells: one-, two- and
//! three-hit bricks in the accent, "good" and warning colours, fading
//! towards the background as they wear, with a dot per hit left beyond the
//! first (so wear reads without colour); unbreakable ones grey with two
//! rivets. The paddle in the text colour (its top in the "good" colour
//! while it catches), balls 3 × 3, capsules rounded with their sign: a bar
//! with ends (wider), S (slower), three dots (split), C (catch), + (life).
//!
//! Partial redraw: a frame repaints only the brick rows whose cells
//! changed and, for each ball, capsule and the paddle that moved, the
//! rectangle covering where it was and where it is; each such region is
//! cleared and everything in it drawn again in order, clipped to it, so a
//! frame drawn in parts equals one drawn whole. The status line is drawn
//! when its contents change; a change of phase or view (served, cleared,
//! paused, over) draws the whole screen with its banner.

use orpheus_games::bricks::{
    BALL, BRICK_H, BRICK_W, BRICKS_X, BRICKS_Y, Bricks, CAP_H, CAP_W, CARRY, COLS, MAX_BALLS, MAX_CAPS, ONE,
    PADDLE_H, PADDLE_Y, Phase, Power, ROWS, SOLID, hits_left, strength,
};
use orpheus_ui::color::{Color, blend};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::nav::Dirty;
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;

use super::game::{View, banner, fnv, fnv_u32};
use super::games::push_points;
use crate::strings::{Lang, Str};

/// The field's top on the screen (under the status line).
pub const TOP: i16 = 12;
const W: i16 = 128;
const SPRITES: usize = MAX_BALLS + MAX_CAPS + 1;

/// What a frame drew: keys of the whole and of the status line, of each
/// brick row, and where each ball, capsule and the paddle were.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BricksDrawn {
    whole: u32,
    info: u32,
    rows: [u32; ROWS],
    sprites: [Rect; SPRITES],
    /// The paddle showed the catch power-up.
    catch: bool,
}

/// Everything the Bricks screen draws from.
pub struct BricksView<'a> {
    pub b: &'a Bricks,
    pub view: View,
    pub new_best: bool,
}

/// Capsule signs, 5 × 3 pixels (bit 4 left).
const SIGNS: [[u8; 3]; 5] = [
    [0b10001, 0b11111, 0b10001],
    [0b00110, 0b00100, 0b01100],
    [0b00000, 0b10101, 0b00000],
    [0b01110, 0b01000, 0b01110],
    [0b00100, 0b01110, 0b00100],
];

fn cell_rect(r: usize, c: usize) -> Rect {
    Rect::new(
        (BRICKS_X + c as i32 * BRICK_W) as i16,
        TOP + (BRICKS_Y + r as i32 * BRICK_H) as i16,
        (BRICK_W - 1) as i16,
        (BRICK_H - 1) as i16,
    )
}

fn row_band(r: usize) -> Rect {
    Rect::new(0, TOP + (BRICKS_Y + r as i32 * BRICK_H) as i16, W, BRICK_H as i16)
}

fn px(v: i32) -> i16 {
    i16::try_from(v >> 8).unwrap_or(i16::MAX)
}

/// The colour of a power-up's capsule.
fn power_color(g: &Gfx<'_>, p: Power) -> Color {
    let pal = &g.pal;
    match p {
        Power::Wide => pal.get(Role::Accent),
        Power::Slow => pal.get(Role::Good),
        Power::Split => pal.get(Role::Warning),
        Power::Catch => pal.mix(Role::Accent, Role::Text, 120),
        Power::Life => pal.mix(Role::Warning, Role::Good, 128),
    }
}

impl BricksView<'_> {
    fn whole_key(&self) -> u32 {
        let b = self.b;
        let mut h = 0x811C_9DC5;
        for v in [self.view as u8, u8::from(self.new_best), b.phase() as u8] {
            h = fnv(h, v);
        }
        fnv_u32(h, u32::from(b.level()))
    }

    fn info_key(&self) -> u32 {
        let b = self.b;
        let h = fnv_u32(0x811C_9DC5, b.score());
        fnv(fnv_u32(h, u32::from(b.level())), b.lives())
    }

    fn row_key(&self, r: usize) -> u32 {
        (0..COLS).fold(0x811C_9DC5, |h, c| fnv(h, self.b.cell(r, c) & !CARRY))
    }

    fn sprites(&self) -> [Rect; SPRITES] {
        let b = self.b;
        let mut out = [Rect::EMPTY; SPRITES];
        for (o, ball) in out.iter_mut().zip(b.balls()) {
            if ball.live() {
                let (x, y) = ball.px();
                *o = Rect::new(x, TOP + y, BALL as i16, BALL as i16);
            }
        }
        for (o, c) in out[MAX_BALLS..].iter_mut().zip(b.capsules()) {
            if c.kind != 0 {
                *o = Rect::new(i16::from(c.x), TOP + px(c.y), CAP_W as i16, CAP_H as i16);
            }
        }
        let (x, w) = b.paddle();
        out[SPRITES - 1] = Rect::new(px(x), TOP + PADDLE_Y as i16, i16::from(w), PADDLE_H as i16);
        out
    }

    /// Draw what changed since `drawn`; returns what is on the screen now.
    pub fn draw(&self, g: &mut Gfx<'_>, l: Lang, dirty: Dirty, drawn: Option<&BricksDrawn>) -> BricksDrawn {
        let mut rows = [0u32; ROWS];
        for (r, k) in rows.iter_mut().enumerate() {
            *k = self.row_key(r);
        }
        let now = BricksDrawn {
            whole: self.whole_key(),
            info: self.info_key(),
            rows,
            sprites: self.sprites(),
            catch: self.b.catching() > 0,
        };
        if dirty != Dirty::All && drawn == Some(&now) {
            return now;
        }
        let full =
            dirty == Dirty::All || drawn.is_none_or(|d| d.whole != now.whole) || self.view != View::Playing;
        let Some(old) = drawn.filter(|_| !full) else {
            g.clear(Role::Bg);
            self.draw_info(g, l);
            self.paint(g, l, Rect::new(0, TOP, W, 128 - TOP));
            self.draw_banner(g, l);
            return now;
        };
        if old.info != now.info {
            self.draw_info(g, l);
        }
        for r in 0..ROWS {
            if old.rows[r] != now.rows[r] {
                self.paint(g, l, row_band(r));
            }
        }
        for k in 0..SPRITES {
            if old.sprites[k] != now.sprites[k] {
                self.paint(g, l, old.sprites[k].union(&now.sprites[k]));
            }
        }
        // The catch power-up shows on the paddle.
        if old.catch != now.catch {
            self.paint(g, l, now.sprites[SPRITES - 1]);
        }
        now
    }

    /// Clear `r` and draw everything in it again, clipped to it.
    fn paint(&self, g: &mut Gfx<'_>, l: Lang, r: Rect) {
        if r.is_empty() {
            return;
        }
        let b = self.b;
        g.with_clip(r, |g| {
            g.fill_role(r, Role::Bg);
            for row in 0..ROWS {
                if !row_band(row).touches(&r) {
                    continue;
                }
                for col in 0..COLS {
                    let cr = cell_rect(row, col);
                    if cr.touches(&r) {
                        draw_brick(g, cr, b.cell(row, col));
                    }
                }
            }
            if b.phase() == Phase::Serve && self.view == View::Playing {
                let dim = g.c(Role::TextDim);
                g.text_in(0, TOP + 84, W, l.get(Str::GameOkToLaunch), FontId::Ui, dim, Align::Center);
            }
            for c in b.capsules() {
                if let Some(p) = c.power() {
                    let cr = Rect::new(i16::from(c.x), TOP + px(c.y), CAP_W as i16, CAP_H as i16);
                    if cr.touches(&r) {
                        draw_capsule(g, cr, p);
                    }
                }
            }
            let (x, w) = b.paddle();
            let pr = Rect::new(px(x), TOP + PADDLE_Y as i16, i16::from(w), PADDLE_H as i16);
            if pr.touches(&r) {
                let text = g.c(Role::Text);
                g.fill(pr, text);
                let ends = g.pal.mix(Role::Text, Role::Bg, 110);
                g.vline(pr.x, pr.y, pr.h, ends);
                g.vline(pr.right() - 1, pr.y, pr.h, ends);
                if b.catching() > 0 {
                    let good = g.c(Role::Good);
                    g.hline(pr.x + 1, pr.y, pr.w - 2, good);
                }
            }
            for ball in b.balls() {
                if ball.live() {
                    let (bx, by) = ball.px();
                    let br = Rect::new(bx, TOP + by, BALL as i16, BALL as i16);
                    if br.touches(&r) {
                        draw_ball(g, br);
                    }
                }
            }
        });
    }

    fn draw_info(&self, g: &mut Gfx<'_>, l: Lang) {
        let b = self.b;
        g.fill_role(Rect::new(0, 0, W, TOP), Role::Bg);
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let mut v = StrBuf::<24>::new();
        push_points(&mut v, b.score());
        g.text(2, 9, v.as_str(), FontId::UiBold, text);
        v.clear();
        v.push_str(l.get(Str::Level));
        v.push(' ');
        v.push_u32(u32::from(b.level()));
        g.text_in(40, 9, 48, v.as_str(), FontId::Ui, dim, Align::Center);
        // Lives as balls, right-aligned.
        let n = i16::from(b.lives().min(9));
        for i in 0..n {
            draw_ball(g, Rect::new(W - 5 - i * 5, 4, BALL as i16, BALL as i16));
        }
        let line = g.pal.mix(Role::Bg, Role::TextDim, 90);
        g.hline(0, TOP - 1, W, line);
    }

    fn draw_banner(&self, g: &mut Gfx<'_>, l: Lang) {
        let b = self.b;
        let r = Rect::new(3, 42, 122, 50);
        let mut v = StrBuf::<24>::new();
        match (self.view, b.phase()) {
            (View::Paused, _) => banner(
                g,
                r,
                &[
                    (l.get(Str::GamePaused), FontId::UiBold, Role::Text),
                    (l.get(Str::GameOkToContinue), FontId::Ui, Role::TextDim),
                ],
            ),
            (View::Ended, _) | (_, Phase::Over) => {
                push_points(&mut v, b.score());
                let last = if self.new_best {
                    (l.get(Str::GameNewBest), FontId::UiBold, Role::Good)
                } else {
                    (l.get(Str::GameOkNewGame), FontId::Ui, Role::TextDim)
                };
                banner(
                    g,
                    r,
                    &[
                        (l.get(Str::GameOver), FontId::UiBold, Role::Text),
                        (v.as_str(), FontId::Ui, Role::Text),
                        last,
                    ],
                );
            }
            (_, Phase::Cleared) => {
                v.push_str(l.get(Str::Level));
                v.push(' ');
                v.push_u32(u32::from(b.level()));
                banner(
                    g,
                    Rect::new(3, 46, 122, 36),
                    &[
                        (l.get(Str::GameLevelCleared), FontId::UiBold, Role::Good),
                        (v.as_str(), FontId::Ui, Role::Text),
                    ],
                );
            }
            _ => {}
        }
    }
}

/// A brick in its cell: colour by the hits it took when new, faded by
/// wear; a dot per hit left beyond the first; unbreakable: grey, riveted.
fn draw_brick(g: &mut Gfx<'_>, r: Rect, cell: u8) {
    let left = hits_left(cell);
    if left == 0 {
        return;
    }
    if cell == SOLID {
        let c = g.pal.mix(Role::TextDim, Role::Surface, 90);
        g.fill(r, c);
        let lit = blend(c, g.c(Role::Text), 90);
        g.hline(r.x, r.y, r.w, lit);
        let rivet = g.pal.mix(Role::Bg, Role::TextDim, 60);
        g.pixel(r.x + 1, r.y + 2, rivet);
        g.pixel(r.right() - 2, r.y + 2, rivet);
        return;
    }
    let s = strength(cell);
    let base = match s {
        1 => g.c(Role::Accent),
        2 => g.c(Role::Good),
        _ => g.c(Role::Warning),
    };
    let worn = 255 - 70 * (s.saturating_sub(left));
    let c = blend(g.c(Role::Bg), base, worn);
    g.fill(r, c);
    let lit = blend(c, g.c(Role::OnAccent), 80);
    g.hline(r.x, r.y, r.w, lit);
    let dot = g.c(Role::Bg);
    let mid = r.x + r.w / 2;
    match left {
        2 => g.pixel(mid, r.y + 2, dot),
        3 => {
            g.pixel(mid - 2, r.y + 2, dot);
            g.pixel(mid + 2, r.y + 2, dot);
        }
        _ => {}
    }
}

fn draw_ball(g: &mut Gfx<'_>, r: Rect) {
    let text = g.c(Role::Text);
    let edge = g.pal.mix(Role::Bg, Role::Text, 150);
    g.fill(r, edge);
    g.hline(r.x + 1, r.y, 1, text);
    g.fill(Rect::new(r.x, r.y + 1, 3, 1), text);
    g.hline(r.x + 1, r.y + 2, 1, text);
}

fn draw_capsule(g: &mut Gfx<'_>, r: Rect, p: Power) {
    let c = power_color(g, p);
    g.fill_round(r, 2, c);
    let ink = g.c(Role::Bg);
    for (dy, bits) in SIGNS[usize::from(p as u8 - 1)].iter().enumerate() {
        for dx in 0..5 {
            if bits & (0b10000 >> dx) != 0 {
                g.pixel(r.x + 1 + dx, r.y + 1 + dy as i16, ink);
            }
        }
    }
}

// `px` turns the engine's 1/256 px into pixels.
const _: () = assert!(ONE == 256);
