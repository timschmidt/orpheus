//! Solitaire on 128 × 128 (05 §7.13, GAME-2): drawing.
//!
//! Full screen. An info line (score, bank or moves; the timer) over the
//! top row: the stock, the waste (draw three: the top three fanned), the
//! four foundations; below it the seven columns, 18 px apart, cards
//! 17 × 20 px. A card shows its rank (own 3 × 5 digits and letters) and
//! suit (own 5 × 5 shapes, a 7 × 7 one on a card that is fully seen) in
//! its top band; the suits differ in shape as well as colour (black suits
//! in the text colour, red in the warning colour), so they read without
//! colour too (the Amber theme, colour-blind players). Columns fan face-
//! down cards 3 px and face-up cards 8 px apart and close up when long
//! (face-down to 1 px, then face-up down to 4 px).
//!
//! The cursor is an accent frame around the cards it covers; cards picked
//! up have a frame of the "good" colour where they lie, and a refused
//! place flashes the target in the warning colour. Only the piles whose
//! look changed are drawn again.

use orpheus_games::solitaire::{
    self, Card, FOUNDATION, PILES, STOCK, Scoring, Solitaire, TABLEAU, WASTE, is_foundation, is_tableau,
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

/// Column pitch, card size, rows.
pub const PITCH: i16 = 18;
pub const CARD_W: i16 = 17;
pub const CARD_H: i16 = 20;
const X0: i16 = 1;
/// The info line's rows (the top row's frames start below them).
const INFO_H: i16 = 11;
const TOP_Y: i16 = 12;
pub const TAB_Y: i16 = 34;
const BOTTOM: i16 = 128;
/// Fan offsets (px): face down, face up; and the smallest they close to.
const DOWN_STEP: i16 = 3;
const UP_STEP: i16 = 8;
const UP_MIN: i16 = 5;
/// The waste's fan (draw three).
const WASTE_STEP: i16 = 9;

/// Ranks, 3 × 5 pixels, a row per 3 bits (bit 2 left): A 2 … 9, J, Q, K
/// (10 is drawn as a narrow 1 and a 0).
const RANKS: [[u8; 5]; 13] = [
    [2, 5, 7, 5, 5],
    [7, 1, 7, 4, 7],
    [7, 1, 3, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 2, 2, 2],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
    [7, 5, 5, 5, 7],
    [1, 1, 1, 5, 7],
    [2, 5, 5, 6, 3],
    [5, 5, 6, 5, 5],
];

/// Suits, 5 × 5 (bit 4 left) and 7 × 7 (bit 6 left): spades (a point
/// with a stem), hearts (two lobes, a point down), clubs (three lobes with
/// gaps, a stem), diamonds (a lozenge).
const SUITS5: [[u8; 5]; 4] = [
    [0b00100, 0b01110, 0b11111, 0b00100, 0b01110],
    [0b11011, 0b11111, 0b11111, 0b01110, 0b00100],
    [0b00100, 0b01110, 0b10101, 0b11111, 0b00100],
    [0b00100, 0b01110, 0b11111, 0b01110, 0b00100],
];
const SUITS7: [[u8; 7]; 4] = [
    [0b0001000, 0b0011100, 0b0111110, 0b1111111, 0b1111111, 0b0001000, 0b0011100],
    [0b0110110, 0b1111111, 0b1111111, 0b1111111, 0b0111110, 0b0011100, 0b0001000],
    [0b0011100, 0b0011100, 0b1101011, 0b1111111, 0b1101011, 0b0001000, 0b0011100],
    [0b0001000, 0b0011100, 0b0111110, 0b1111111, 0b0111110, 0b0011100, 0b0001000],
];

fn bits(g: &mut Gfx<'_>, x: i16, y: i16, rows: &[u8], w: i16, c: Color) {
    for (r, &b) in rows.iter().enumerate() {
        for col in 0..w {
            if b & (1 << (w - 1 - col)) != 0 {
                g.pixel(x + col, y + i16::try_from(r).unwrap_or(0), c);
            }
        }
    }
}

/// The ink of a card: red suits in the warning colour, black in the text
/// colour.
fn ink(g: &Gfx<'_>, c: Card) -> Color {
    if solitaire::red(c) { g.c(Role::Warning) } else { g.c(Role::Text) }
}

fn face(g: &Gfx<'_>) -> Color {
    g.c(Role::Surface)
}

fn edge(g: &Gfx<'_>) -> Color {
    g.pal.mix(Role::Surface, Role::TextDim, 120)
}

/// A card face up at `x`, `y`; `whole`: nothing covers it (the big suit).
fn card_up(g: &mut Gfx<'_>, x: i16, y: i16, c: Card, whole: bool) {
    let r = Rect::new(x, y, CARD_W, CARD_H);
    let (f, e) = (face(g), edge(g));
    g.fill_round(r, 2, f);
    g.stroke_round(r, 2, e);
    let ink = ink(g, c);
    let rank = solitaire::rank(c);
    if rank == 9 {
        // 10: a narrow 1 and a 0.
        bits(g, x + 2, y + 1, &[1, 1, 1, 1, 1], 1, ink);
        bits(g, x + 4, y + 1, &RANKS[9], 3, ink);
    } else {
        bits(g, x + 2, y + 1, &RANKS[usize::from(rank)], 3, ink);
    }
    let s = usize::from(solitaire::suit(c));
    bits(g, x + CARD_W - 7, y + 1, &SUITS5[s], 5, ink);
    if whole {
        bits(g, x + (CARD_W - 7) / 2, y + 10, &SUITS7[s], 7, ink);
    }
}

/// A card face down.
fn card_down(g: &mut Gfx<'_>, x: i16, y: i16, h: i16) {
    let r = Rect::new(x, y, CARD_W, h);
    let back = g.pal.mix(Role::Accent, Role::Bg, 110);
    let line = g.pal.mix(Role::Accent, Role::Bg, 40);
    g.fill_round(r, 2, back);
    // A diagonal hatch, 3 px apart, inside a 2 px margin.
    for yy in 2..h - 2 {
        for xx in 2..CARD_W - 2 {
            if (xx + yy) % 3 == 0 {
                g.pixel(x + xx, y + yy, line);
            }
        }
    }
    let e = g.pal.mix(Role::Accent, Role::Bg, 70);
    g.stroke_round(r, 2, e);
}

/// An empty pile: a dim outline, with a mark.
fn slot(g: &mut Gfx<'_>, x: i16, y: i16, mark: Option<&[u8; 5]>, mark_w: i16) {
    let r = Rect::new(x, y, CARD_W, CARD_H);
    let c = g.pal.mix(Role::Bg, Role::TextDim, 90);
    g.stroke_round(r, 2, c);
    if let Some(m) = mark {
        bits(g, x + (CARD_W - mark_w) / 2, y + (CARD_H - 5) / 2, m, mark_w, c);
    }
}

/// The stock's empty-pile marks: a turn-back ring (passes left), a cross.
const RING: [u8; 5] = [0b01110, 0b10001, 0b10101, 0b10001, 0b01110];
const CROSS: [u8; 5] = [0b10001, 0b01010, 0b00100, 0b01010, 0b10001];

/// The piles left and right of `p` in its row (they share a gap).
fn row_neighbours(p: u8) -> [Option<u8>; 2] {
    // The top row: stock, waste, the four foundations.
    const TOP: [u8; 6] = [STOCK, WASTE, FOUNDATION, FOUNDATION + 1, FOUNDATION + 2, FOUNDATION + 3];
    let (row, i): (&[u8], usize) = if is_tableau(p) {
        (&[6, 7, 8, 9, 10, 11, 12], usize::from(p - TABLEAU))
    } else {
        (&TOP, TOP.iter().position(|&q| q == p).unwrap_or(0))
    };
    [i.checked_sub(1).map(|k| row[k]), row.get(i + 1).copied()]
}

fn col_x(i: i16) -> i16 {
    X0 + i * PITCH
}

/// The area a pile is drawn in.
#[must_use]
pub fn pile_rect(p: u8) -> Rect {
    match p {
        STOCK => Rect::new(col_x(0), TOP_Y, CARD_W, CARD_H),
        WASTE => Rect::new(col_x(1), TOP_Y, PITCH + CARD_W, CARD_H),
        _ if is_foundation(p) => Rect::new(col_x(3 + i16::from(p - FOUNDATION)), TOP_Y, CARD_W, CARD_H),
        _ => Rect::new(col_x(i16::from(p - TABLEAU)), TAB_Y, CARD_W, BOTTOM - TAB_Y),
    }
}

/// Fan offsets of a column with `down` face-down and `up` face-up cards:
/// they close up until the column fits above the bottom.
#[must_use]
pub fn fan(down: usize, up: usize) -> (i16, i16) {
    let avail = BOTTOM - TAB_Y - CARD_H;
    let d = i16::try_from(down).unwrap_or(i16::MAX);
    let gaps = i16::try_from(up.saturating_sub(1)).unwrap_or(i16::MAX);
    let need = |od: i16, ou: i16| d.saturating_mul(od).saturating_add(gaps.saturating_mul(ou));
    let mut od = DOWN_STEP;
    while od > 1 && need(od, UP_STEP) > avail {
        od -= 1;
    }
    let mut ou = UP_STEP;
    if need(od, ou) > avail && gaps > 0 {
        ou = ((avail - d * od) / gaps).clamp(UP_MIN, UP_STEP);
    }
    (od, ou)
}

/// The y of each card of a column (top of the card), bottom card first.
fn column_ys(b: &solitaire::Board, p: u8, out: &mut [i16; 20]) -> usize {
    let n = b.len(p).min(out.len());
    let down = b.down(p);
    let (od, ou) = fan(down, b.face_up(p));
    let mut y = TAB_Y;
    for (k, slot) in out.iter_mut().enumerate().take(n) {
        *slot = y;
        y += if k < down { od } else { ou };
    }
    n
}

/// The cards of the waste shown, oldest first: one, or the top three
/// when drawing three.
fn waste_shown(s: &Solitaire) -> &[Card] {
    let w = s.board().pile(WASTE);
    let k = if s.config().draw == 3 { 3 } else { 1 };
    &w[w.len().saturating_sub(k)..]
}

/// The rectangle of the top `n` cards of pile `p` (the cursor's frame).
#[must_use]
pub fn cards_rect(s: &Solitaire, p: u8, n: usize) -> Rect {
    let b = s.board();
    if is_tableau(p) && b.len(p) > 0 {
        let mut ys = [0i16; 20];
        let len = column_ys(b, p, &mut ys);
        let first = len.saturating_sub(n.max(1));
        let y0 = ys[first];
        let y1 = ys[len - 1] + CARD_H;
        return Rect::new(col_x(i16::from(p - TABLEAU)), y0, CARD_W, y1 - y0);
    }
    if p == WASTE {
        let shown = waste_shown(s).len();
        let x = col_x(1) + WASTE_STEP * i16::try_from(shown.saturating_sub(1)).unwrap_or(0);
        return Rect::new(x, TOP_Y, CARD_W, CARD_H);
    }
    let r = pile_rect(p);
    Rect::new(r.x, r.y, CARD_W, CARD_H)
}

/// Everything the Solitaire screen draws from.
pub struct SolitaireView<'a> {
    pub s: &'a Solitaire,
    pub view: View,
    pub new_best: bool,
    pub timer: bool,
    /// Casino scoring: the bank as it stands with this game.
    pub bank: i32,
    pub refused: bool,
}

const INFO: usize = PILES;
const WHOLE: usize = PILES + 1;

impl SolitaireView<'_> {
    fn pile_key(&self, p: u8) -> u32 {
        let s = self.s;
        let b = s.board();
        let mut h = 0x811C_9DC5;
        for &c in b.pile(p) {
            h = fnv(h, c);
        }
        h = fnv(h, b.down(p) as u8);
        let (cur, depth) = s.cursor();
        if cur == p {
            h = fnv(h, 0x40 | depth);
            h = fnv(h, u8::from(self.refused));
            // Holding: the target mark under the cursor.
            h = fnv(h, u8::from(s.held().is_some()));
        }
        if let Some((hp, n)) = s.held()
            && hp == p
        {
            h = fnv(h, 0x80 | n);
        }
        if p == STOCK {
            h = fnv(h, s.passes_left().unwrap_or(0xFF));
            h = fnv(h, u8::from(b.recycle_left(&s.config())));
        }
        h
    }

    fn info_key(&self) -> u32 {
        let s = self.s;
        let mut h = fnv_u32(0x811C_9DC5, s.score() as u32);
        h = fnv_u32(h, self.bank as u32);
        h = fnv_u32(h, s.moves());
        if self.timer {
            h = fnv_u32(h, s.steps() / orpheus_games::STEP_HZ);
        }
        h
    }

    fn whole_key(&self) -> u32 {
        let h = fnv(0x811C_9DC5, self.view as u8);
        let h = fnv(h, u8::from(self.new_best));
        fnv(h, u8::from(self.timer))
    }

    /// Draw what changed; returns the keys of what is on screen.
    pub fn draw(&self, g: &mut Gfx<'_>, l: Lang, dirty: Dirty, drawn: Option<[u32; 15]>) -> [u32; 15] {
        let mut keys = [0u32; 15];
        for p in 0..PILES as u8 {
            keys[usize::from(p)] = self.pile_key(p);
        }
        keys[INFO] = self.info_key();
        keys[WHOLE] = self.whole_key();
        let full = dirty == Dirty::All
            || drawn.is_none_or(|d| d[WHOLE] != keys[WHOLE])
            || (self.view != View::Playing && drawn != Some(keys));
        if !full && drawn == Some(keys) {
            return keys;
        }
        if full {
            g.clear(Role::Bg);
        }
        let old = drawn.unwrap_or([0; 15]);
        if full || old[INFO] != keys[INFO] {
            self.draw_info(g, l);
        }
        // A pile that changed is cleared with the gap around it (frames
        // reach into it), so its neighbours in the row are drawn again
        // too (their frames share the gap).
        let changed = |p: u8| full || old[usize::from(p)] != keys[usize::from(p)];
        let mut redraw = [false; PILES];
        for p in 0..PILES as u8 {
            if changed(p) {
                redraw[usize::from(p)] = true;
                for q in row_neighbours(p).into_iter().flatten() {
                    redraw[usize::from(q)] = true;
                }
            }
        }
        if !full {
            for p in 0..PILES as u8 {
                if changed(p) {
                    let r = pile_rect(p);
                    g.fill_role(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2), Role::Bg);
                } else if redraw[usize::from(p)] {
                    g.fill_role(pile_rect(p), Role::Bg);
                }
            }
        }
        for p in 0..PILES as u8 {
            if redraw[usize::from(p)] {
                self.draw_pile(g, p);
            }
        }
        if self.view != View::Playing {
            self.draw_banner(g, l);
        }
        keys
    }

    fn draw_info(&self, g: &mut Gfx<'_>, l: Lang) {
        let s = self.s;
        g.fill_role(Rect::new(0, 0, 128, INFO_H), Role::Bg);
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let mut v = StrBuf::<24>::new();
        let label = match s.config().scoring {
            Scoring::Standard => {
                push_points(&mut v, u32::try_from(s.score()).unwrap_or(0));
                Str::GameScore
            }
            Scoring::Casino => {
                if self.bank < 0 {
                    v.push('−');
                }
                push_points(&mut v, self.bank.unsigned_abs());
                Str::GameBank
            }
            Scoring::None => {
                v.push_u32(s.moves());
                Str::GameMoves
            }
        };
        let x = g.text(2, 9, l.get(label), FontId::Ui, dim);
        g.text(x + 3, 9, v.as_str(), FontId::UiBold, text);
        if self.timer {
            let secs = orpheus_games::steps_to_ms(s.steps()) / 1000;
            v.clear();
            v.push_u32(secs / 60);
            v.push(':');
            v.push_u32_padded(secs % 60, 2);
            g.text_in(64, 9, 62, v.as_str(), FontId::Ui, text, Align::Right);
        }
    }

    /// A frame of two pixels: the cards' own border and the gap around
    /// them (the rank and suit inside stay clear). Plain strokes, not
    /// blended: a pile drawn again over an old frame gives the same pixels
    /// as one drawn on a clear screen.
    fn frame(g: &mut Gfx<'_>, r: Rect, c: Color) {
        g.stroke(r, c);
        g.stroke(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2), c);
    }

    fn draw_pile(&self, g: &mut Gfx<'_>, p: u8) {
        let s = self.s;
        let b = s.board();
        let r = pile_rect(p);
        match p {
            STOCK => {
                if b.len(STOCK) > 0 {
                    card_down(g, r.x, r.y, CARD_H);
                } else if b.len(WASTE) == 0 {
                    slot(g, r.x, r.y, None, 0);
                } else if b.recycle_left(&s.config()) {
                    slot(g, r.x, r.y, Some(&RING), 5);
                } else {
                    slot(g, r.x, r.y, Some(&CROSS), 5);
                }
            }
            WASTE => {
                let shown = waste_shown(s);
                if shown.is_empty() {
                    slot(g, r.x, r.y, None, 0);
                }
                for (k, &c) in shown.iter().enumerate() {
                    let x = r.x + WASTE_STEP * i16::try_from(k).unwrap_or(0);
                    let top = k + 1 == shown.len();
                    card_up(g, x, r.y, c, top);
                    if !top {
                        // Covered but for its left strip: the suit under
                        // the rank.
                        let ink = ink(g, c);
                        bits(g, x + 2, r.y + 8, &SUITS5[usize::from(solitaire::suit(c))], 5, ink);
                    }
                }
            }
            _ if is_foundation(p) => match b.top(p) {
                Some(c) => card_up(g, r.x, r.y, c, true),
                None => slot(g, r.x, r.y, Some(&RANKS[0]), 3),
            },
            _ => {
                let mut ys = [0i16; 20];
                let n = column_ys(b, p, &mut ys);
                if n == 0 {
                    slot(g, r.x, r.y, None, 0);
                }
                let down = b.down(p);
                let pile = b.pile(p);
                for k in 0..n {
                    let y = ys[k];
                    if k < down {
                        let h = if k + 1 < n { (ys[k + 1] - y + 3).min(CARD_H) } else { CARD_H };
                        card_down(g, r.x, y, h);
                    } else {
                        card_up(g, r.x, y, pile[k], k + 1 == n);
                    }
                }
            }
        }
        // The cards held, then the cursor.
        if let Some((hp, hn)) = s.held()
            && hp == p
        {
            let c = g.c(Role::Good);
            Self::frame(g, cards_rect(s, p, usize::from(hn)), c);
        }
        let (cur, depth) = s.cursor();
        if cur == p && self.view == View::Playing {
            let n = if is_tableau(p) && s.held().is_none() { usize::from(depth) } else { 1 };
            let c = if self.refused { g.c(Role::Warning) } else { g.c(Role::Accent) };
            let fr = cards_rect(s, p, n);
            Self::frame(g, fr, c);
            // A thicker mark under a pile the cards would go to.
            if s.held().is_some_and(|(hp, _)| hp != p) {
                let bar = blend(g.c(Role::Bg), c, 200);
                g.hline(fr.x + 2, fr.y + fr.h - 3, fr.w - 4, bar);
            }
        }
    }

    fn draw_banner(&self, g: &mut Gfx<'_>, l: Lang) {
        let s = self.s;
        let r = Rect::new(8, 42, 112, 46);
        let mut v = StrBuf::<24>::new();
        match self.view {
            View::Paused => banner(
                g,
                Rect::new(8, 48, 112, 34),
                &[
                    (l.get(Str::GamePaused), FontId::UiBold, Role::Text),
                    (l.get(Str::GameOkToContinue), FontId::Ui, Role::TextDim),
                ],
            ),
            View::Ended => {
                let (title, role) =
                    if s.won() { (Str::GameWon, Role::Good) } else { (Str::GameNoMoves, Role::Text) };
                match s.config().scoring {
                    Scoring::Standard => push_points(&mut v, u32::try_from(s.score()).unwrap_or(0)),
                    Scoring::Casino => {
                        v.push_str(l.get(Str::GameBank));
                        v.push(' ');
                        if self.bank < 0 {
                            v.push('−');
                        }
                        push_points(&mut v, self.bank.unsigned_abs());
                    }
                    Scoring::None => super::games::push_time(&mut v, orpheus_games::steps_to_ms(s.steps())),
                }
                let last = if self.new_best {
                    (l.get(Str::GameNewBest), FontId::UiBold, Role::Good)
                } else {
                    (l.get(Str::GameOkNewGame), FontId::Ui, Role::TextDim)
                };
                banner(
                    g,
                    r,
                    &[(l.get(title), FontId::UiBold, role), (v.as_str(), FontId::Ui, Role::Text), last],
                );
            }
            View::Playing | View::Loading => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits() {
        // Seven columns on 128 px.
        assert!(col_x(6) + CARD_W <= 128);
        // The top row: stock, waste fan of three, four foundations.
        assert!(col_x(1) + 2 * WASTE_STEP + CARD_W <= col_x(3));
        // The longest column there can be (six face down, king to ace)
        // fits above the bottom, its cards' bands readable.
        let (od, ou) = fan(6, 13);
        assert!(6 * od + 12 * ou + CARD_H <= BOTTOM - TAB_Y, "{od} {ou}");
        assert!(ou >= UP_MIN);
        assert_eq!(fan(6, 1), (DOWN_STEP, UP_STEP));
        assert_eq!(fan(0, 0), (DOWN_STEP, UP_STEP));
    }

    #[test]
    fn suit_shapes_differ() {
        for a in 0..4 {
            for b in a + 1..4 {
                assert_ne!(SUITS5[a], SUITS5[b]);
                assert_ne!(SUITS7[a], SUITS7[b]);
                // At least three pixels apart in the small shape.
                let d: u32 = (0..5).map(|r| (SUITS5[a][r] ^ SUITS5[b][r]).count_ones()).sum();
                assert!(d >= 3, "suits {a} and {b}: {d}");
            }
        }
        assert!(pile_rect(STOCK).x < pile_rect(WASTE).x);
    }
}
