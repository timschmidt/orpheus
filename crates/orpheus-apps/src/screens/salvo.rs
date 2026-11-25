//! Salvo on 128 × 128 (05 §7.13, GAME-4/5): drawing.
//!
//! Full screen. Two info lines on a band at the top: the player whose turn
//! it is (in its colour), the barrel's angle and the power, the wind (an
//! arrow as long as it is strong, its speed in m/s; with gusts the air as
//! it moves now); below them the weapon and its rounds, and the round (or
//! the turn timer, or "thinking" on a computer's turn). Under the band the
//! world: one column of ground per pixel (a lighter top edge), water over
//! the islands' sea, the tanks (body, barrel, their number above in the
//! player's colour and a health bar), shells, the last blast, smoke puffs,
//! and the aim guide (the last shot's path or a short preview, dots).
//! The view follows the action across worlds wider than the screen; the
//! overview (hold OK) squeezes the whole width into 128 px. A shell above
//! the screen shows as a mark on the band's edge.
//!
//! Overlays: the weapon list (Menu), "Pass to Player n", the round's
//! winner, the shop, the end of the game, paused. The screen is drawn
//! again whenever anything it shows changed (a key over the state).

use orpheus_games::salvo::{
    BLAST_STEPS, BOMBLET, Hit, Kind, Phase, SPECS, Salvo, TANK_H, TANK_HALF, WEAPONS, WORLD_H,
};
use orpheus_ui::color::Color;
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;

use super::game::{View, banner, fnv, fnv_u32};
use super::games::push_points;
use crate::strings::{Lang, Str};

/// The info band's height.
pub const HUD_H: i16 = 22;
const W: i16 = 128;

/// Digits 0–9, 3 × 5 pixels (bit 2 left), for the tanks' numbers.
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 3, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 2, 2, 2],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

/// The weapons' names.
pub const WEAPON_NAMES: [Str; WEAPONS] = [
    Str::WeaponShell,
    Str::WeaponHeavy,
    Str::WeaponCluster,
    Str::WeaponRoller,
    Str::WeaponDigger,
    Str::WeaponDirt,
    Str::WeaponSmoke,
];

/// Player `i`'s colour: roles (every theme), each player its own; the
/// number above the tank tells them apart without colour.
#[must_use]
pub fn player_color(g: &Gfx<'_>, i: usize) -> Color {
    match i {
        0 => g.c(Role::Accent),
        1 => g.c(Role::Warning),
        2 => g.c(Role::Good),
        _ => g.c(Role::Text),
    }
}

fn digit(g: &mut Gfx<'_>, x: i16, y: i16, d: usize, c: Color) {
    for (r, &bits) in DIGITS[d % 10].iter().enumerate() {
        for col in 0..3 {
            if bits & (4 >> col) != 0 {
                g.pixel(x + col, y + r as i16, c);
            }
        }
    }
}

/// Everything the Salvo screen draws from.
pub struct SalvoView<'a> {
    pub s: &'a Salvo,
    pub view: View,
    /// Aim guide: 0 none, 1 last shot, 2 preview.
    pub guide: u8,
    pub new_best: bool,
}

impl SalvoView<'_> {
    /// A key over everything drawn: equal keys, the same picture.
    #[must_use]
    pub fn key(&self) -> u32 {
        let s = self.s;
        let mut h = 0x811C_9DC5;
        for b in [
            self.view as u8,
            self.guide,
            u8::from(self.new_best),
            s.phase() as u8,
            s.turn(),
            s.round(),
            u8::from(s.overview()),
            u8::from(s.fast()),
            s.menu().unwrap_or(0xFF),
            s.shop_row(),
            s.water(),
        ] {
            h = fnv(h, b);
        }
        h = fnv_u32(h, s.camera() as u32);
        h = fnv_u32(h, u32::from(s.timer_left().unwrap_or(0xFFFF)));
        h = fnv_u32(h, s.wind_tenths() as u32);
        h = fnv_u32(h, s.wind().1.to_bits());
        for &c in s.heights() {
            h = fnv(h, c);
        }
        for t in s.tanks() {
            for v in [t.x as u32, t.y as u32, u32::from(t.angle), u32::from(t.power), u32::from(t.health)] {
                h = fnv_u32(h, v);
            }
            h = fnv(h, t.weapon);
            h = fnv_u32(h, t.score as u32);
            h = fnv_u32(h, u32::from(t.credits));
            for &a in &t.ammo {
                h = fnv(h, a);
            }
        }
        for sh in s.shells().iter().filter(|sh| sh.active()) {
            let (x, y) = sh.px();
            h = fnv_u32(h, x as i32 as u32);
            h = fnv_u32(h, y as i32 as u32);
        }
        for p in s.puffs().iter().filter(|p| p.age > 0) {
            let (x, y) = p.px();
            h = fnv_u32(h, (x as u32) << 16 | u32::from(y as u16));
        }
        let b = s.blast();
        if b.shown() {
            h = fnv_u32(h, (b.x as u32) << 16 | u32::from(b.y as u16));
            h = fnv(h, b.age);
        }
        h
    }

    /// The screen x of world x (px).
    fn sx(&self, wx: f32) -> i16 {
        let s = self.s;
        if s.overview() {
            (wx * f32::from(W) / f32::from(s.width())) as i16
        } else {
            (wx - f32::from(s.camera())) as i16
        }
    }

    pub fn draw(&self, g: &mut Gfx<'_>, l: Lang) {
        g.clear(Role::Bg);
        self.draw_world(g);
        self.draw_hud(g, l);
        self.draw_overlay(g, l);
    }

    fn draw_world(&self, g: &mut Gfx<'_>) {
        let s = self.s;
        let w = i32::from(s.width());
        let ground = g.pal.mix(Role::TextDim, Role::Bg, 120);
        let edge = g.pal.mix(Role::Good, Role::TextDim, 150);
        let water = g.pal.mix(Role::Accent, Role::Bg, 110);
        let surf = g.pal.mix(Role::Accent, Role::Text, 90);
        let cam = i32::from(s.camera());
        for x in 0..W {
            // The columns this screen column shows: one, or the overview's
            // share (the highest).
            let (c0, c1) = if s.overview() {
                (
                    i32::from(x) * w / i32::from(W),
                    ((i32::from(x) + 1) * w / i32::from(W)).max(i32::from(x) * w / i32::from(W) + 1),
                )
            } else {
                (cam + i32::from(x), cam + i32::from(x) + 1)
            };
            if c0 >= w {
                continue;
            }
            let h = (c0..c1.min(w)).map(|c| i16::from(s.height(c))).max().unwrap_or(0);
            let top = WORLD_H - h;
            if h > 0 {
                g.vline(x, top, h, ground);
                g.pixel(x, top, edge);
            }
            let wl = i16::from(s.water());
            if wl > h {
                g.vline(x, WORLD_H - wl, wl - h, water);
                g.pixel(x, WORLD_H - wl, surf);
            }
        }
        // Smoke.
        let smoke = g.pal.mix(Role::Bg, Role::TextDim, 190);
        for p in s.puffs().iter().filter(|p| p.age > 0) {
            let (px, py) = p.px();
            let x = self.sx(f32::from(px));
            let r = 3 + i16::try_from(p.age / 150).unwrap_or(0).min(3);
            g.fill_disc(Rect::new(x - r, WORLD_H - py - r, 2 * r, 2 * r), smoke);
        }
        self.draw_guide(g);
        for (i, t) in s.tanks().iter().enumerate() {
            if t.alive() {
                self.draw_tank(g, i);
            }
        }
        // Tanks beyond the view: their number and an arrow at the edge.
        if !s.overview() {
            for (i, t) in s.tanks().iter().enumerate() {
                let x = self.sx(f32::from(t.x));
                if !t.alive() || (0..W).contains(&x) {
                    continue;
                }
                let c = player_color(g, i);
                let y = (WORLD_H - t.y_px() - 8).clamp(HUD_H + 4, WORLD_H - 8);
                // The tip at the edge, the number inside it.
                let left = x < 0;
                for k in 0..3 {
                    let ax = if left { k } else { W - 1 - k };
                    g.vline(ax, y + 2 - k, 2 * k + 1, c);
                }
                digit(g, if left { 4 } else { W - 8 }, y, i + 1, c);
            }
        }
        // Shells, and a mark on the band for one above the screen.
        let ink = g.c(Role::Text);
        for sh in s.shells().iter().filter(|sh| sh.active()) {
            let (px, py) = sh.px();
            let x = self.sx(px);
            let y = WORLD_H - py as i16;
            if y < HUD_H {
                g.fill(Rect::new(x - 1, HUD_H, 3, 2), ink);
            } else if sh.kind == BOMBLET {
                g.pixel(x, y, ink);
            } else {
                g.fill(Rect::new(x, y - 1, 2, 2), ink);
            }
        }
        let b = s.blast();
        if b.shown() {
            let r = i16::from(b.r).max(2);
            let grow = i16::from(b.age.min(6) + 1) * r / 7;
            let fade = u8::try_from(u16::from(b.age) * 255 / u16::from(BLAST_STEPS)).unwrap_or(255);
            let c = if b.age < 8 { g.c(Role::Warning) } else { g.pal.mix(Role::Warning, Role::Bg, fade) };
            let x = self.sx(f32::from(b.x));
            let y = WORLD_H - b.y;
            g.fill_disc(Rect::new(x - grow, y - grow, 2 * grow + 1, 2 * grow + 1), c);
        }
    }

    fn draw_tank(&self, g: &mut Gfx<'_>, i: usize) {
        let s = self.s;
        let t = s.tanks()[i];
        let c = player_color(g, i);
        let x = self.sx(f32::from(t.x));
        let bottom = WORLD_H - t.y_px() - 1;
        // Body (7 × 3) and dome.
        g.fill(Rect::new(x - TANK_HALF, bottom - 2, 2 * TANK_HALF + 1, 3), c);
        g.fill(Rect::new(x - 1, bottom - 3, 3, 1), c);
        // Barrel from the pivot.
        let (cs, sn) = (orpheus_games::salvo::cos_deg(t.angle), orpheus_games::salvo::sin_deg(t.angle));
        let (px, py) = (f32::from(x) + 0.5, f32::from(bottom - TANK_H + 2) + 0.5);
        for k in 1..=5 {
            let kx = (px + cs * k as f32) as i16;
            let ky = (py - sn * k as f32) as i16;
            g.pixel(kx, ky, c);
        }
        // Number and health above.
        let top = bottom - 12;
        digit(g, x - 1, top, i + 1, c);
        let full = i32::from(s.config().health).max(1);
        let len = (i32::from(t.health) * 7 / full).clamp(1, 7) as i16;
        let hc = if i32::from(t.health) * 3 < full { g.c(Role::Warning) } else { g.c(Role::Good) };
        g.fill(Rect::new(x - 3, top + 6, len, 1), hc);
        // The tank whose turn it is: a mark under its number.
        if usize::from(s.turn()) == i && matches!(s.phase(), Phase::Aim | Phase::Think | Phase::Turn) {
            g.hline(x - 3, top - 2, 7, c);
        }
    }

    /// The aim guide: the last shot's path under the wind of then, or the
    /// first part of the aimed shot in still air.
    fn draw_guide(&self, g: &mut Gfx<'_>) {
        let s = self.s;
        if self.guide == 0 || s.phase() != Phase::Aim || s.menu().is_some() {
            return;
        }
        let i = usize::from(s.turn());
        let t = s.tanks()[i];
        let dot = g.c(Role::TextDim);
        // A dot every 0.3 s of the last shot's flight (all of it), or every
        // 0.1 s of the first 2 s of the aimed one.
        let (shell, wind, every, limit) = if self.guide == 1 {
            if t.last_weapon == 0xFF {
                return;
            }
            let sh = s.launch(i, t.last_angle, t.last_power, t.last_weapon);
            (sh, f32::from(t.last_wind) / 2.0, 60, u32::MAX)
        } else {
            (s.launch(i, t.angle, t.power, t.weapon), 0.0, 20, 400)
        };
        let _: Hit = s.simulate(shell, i, wind, None, |k, sh| {
            if k % every == every - 1 {
                let (x, y) = sh.px();
                let y = WORLD_H - y as i16;
                if y >= HUD_H {
                    g.pixel(self.sx(x), y, dot);
                }
            }
            k < limit
        });
    }

    fn draw_hud(&self, g: &mut Gfx<'_>, l: Lang) {
        let s = self.s;
        g.fill_role(Rect::new(0, 0, W, HUD_H), Role::Surface);
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let i = usize::from(s.turn());
        let t = s.tanks()[i];
        let c = player_color(g, i);
        let mut v = StrBuf::<24>::new();
        v.push('P');
        v.push_u32(i as u32 + 1);
        let mut x = g.text(2, 9, v.as_str(), FontId::UiBold, c);
        v.clear();
        v.push_u32(u32::from(t.angle));
        v.push('°');
        x = g.text(x + 4, 9, v.as_str(), FontId::Ui, text);
        v.clear();
        v.push_u32(u32::from(t.power / 10));
        v.push('.');
        v.push_u32(u32::from(t.power % 10));
        g.text(x + 4, 9, v.as_str(), FontId::Ui, text);
        // The wind: an arrow and m/s.
        let (w0, gust) = s.wind();
        let air = w0 + gust;
        let tenths = (air * 10.0) as i32;
        v.clear();
        v.push_u32(tenths.unsigned_abs() / 10);
        v.push('.');
        v.push_u32(tenths.unsigned_abs() % 10);
        let vw = g.measure(v.as_str(), FontId::Ui);
        g.text(W - 2 - vw, 9, v.as_str(), FontId::Ui, text);
        let len = ((4.0 + air.abs() * 0.8) as i16).clamp(4, 14);
        let ax = W - 4 - vw - 16;
        let ay = 5;
        let accent = g.c(Role::Accent);
        if tenths == 0 {
            g.fill(Rect::new(ax + 6, ay - 1, 3, 3), dim);
        } else {
            let (x0, x1) = if air > 0.0 { (ax + 14 - len, ax + 14) } else { (ax, ax + len) };
            g.hline(x0, ay, x1 - x0 + 1, accent);
            let tip = if air > 0.0 { x1 } else { x0 };
            let d = if air > 0.0 { -1 } else { 1 };
            g.pixel(tip + d, ay - 1, accent);
            g.pixel(tip + d, ay + 1, accent);
            g.pixel(tip + 2 * d, ay - 2, accent);
            g.pixel(tip + 2 * d, ay + 2, accent);
        }
        // Second line: weapon, round / timer / thinking.
        let kind = s.config().kinds[i];
        v.clear();
        if kind != Kind::Human && matches!(s.phase(), Phase::Think | Phase::Turn) {
            v.push_str(l.get(Str::GamePlayer));
            v.push(' ');
            v.push_u32(i as u32 + 1);
            v.push(' ');
            v.push_str(l.get(Str::GameThinking));
            g.text(2, 20, v.as_str(), FontId::Ui, dim);
        } else {
            let wn = l.get(WEAPON_NAMES[usize::from(t.weapon) % WEAPONS]);
            let x = g.text(2, 20, wn, FontId::Ui, dim);
            if t.weapon != 0 {
                v.push('×');
                v.push_u32(u32::from(t.rounds(t.weapon)));
                g.text(x + 3, 20, v.as_str(), FontId::Ui, dim);
            }
        }
        v.clear();
        if let Some(secs) = s.timer_left() {
            v.push_u32(u32::from(secs));
            v.push_str(" s");
        } else {
            v.push('R');
            v.push_u32(u32::from(s.round()) + 1);
            v.push('/');
            v.push_u32(u32::from(s.config().rounds));
        }
        if s.fast() {
            v.push_str(" »");
        }
        let vw = g.measure(v.as_str(), FontId::Ui);
        g.text(W - 2 - vw, 20, v.as_str(), FontId::Ui, dim);
    }

    fn draw_overlay(&self, g: &mut Gfx<'_>, l: Lang) {
        let s = self.s;
        match self.view {
            View::Paused => {
                banner(
                    g,
                    Rect::new(8, 48, 112, 34),
                    &[
                        (l.get(Str::GamePaused), FontId::UiBold, Role::Text),
                        (l.get(Str::GameOkToContinue), FontId::Ui, Role::TextDim),
                    ],
                );
                return;
            }
            View::Ended => {
                self.draw_standings(g, l, true);
                return;
            }
            View::Playing | View::Loading => {}
        }
        if let Some(cur) = s.menu() {
            self.draw_menu(g, l, cur);
            return;
        }
        let mut v = StrBuf::<32>::new();
        match s.phase() {
            Phase::Pass => {
                v.push_str(l.get(Str::GamePlayer));
                v.push(' ');
                v.push_u32(u32::from(s.turn()) + 1);
                banner(
                    g,
                    Rect::new(8, 40, 112, 48),
                    &[
                        (l.get(Str::GamePassTo), FontId::Ui, Role::TextDim),
                        (v.as_str(), FontId::UiBold, Role::Accent),
                        (l.get(Str::GameOkToStart), FontId::Ui, Role::TextDim),
                    ],
                );
            }
            Phase::RoundOver => {
                let second = match s.round_winner() {
                    Some(p) => {
                        v.push_str(l.get(Str::GamePlayer));
                        v.push(' ');
                        v.push_u32(u32::from(p) + 1);
                        l.get(Str::GameWinsRound)
                    }
                    None => {
                        v.push_str(l.get(Str::GameNobody));
                        ""
                    }
                };
                let mut r = StrBuf::<24>::new();
                r.push_str(l.get(Str::GameRound));
                r.push(' ');
                r.push_u32(u32::from(s.round()) + 1);
                r.push('/');
                r.push_u32(u32::from(s.config().rounds));
                banner(
                    g,
                    Rect::new(6, 36, 116, 56),
                    &[
                        (r.as_str(), FontId::Ui, Role::TextDim),
                        (v.as_str(), FontId::UiBold, Role::Text),
                        (second, FontId::Ui, Role::Text),
                    ],
                );
            }
            Phase::Shop => self.draw_shop(g, l),
            Phase::Over => self.draw_standings(g, l, false),
            _ => {}
        }
    }

    fn draw_menu(&self, g: &mut Gfx<'_>, l: Lang, cur: u8) {
        let s = self.s;
        let i = usize::from(s.turn());
        let t = s.tanks()[i];
        let r = Rect::new(14, HUD_H + 2, 100, 7 * 13 + 4);
        let surface = g.c(Role::Surface);
        let edge = g.pal.mix(Role::Surface, Role::TextDim, 110);
        g.fill_round(r, 4, surface);
        g.stroke_round(r, 4, edge);
        for (k, name) in WEAPON_NAMES.iter().enumerate() {
            let y = r.y + 2 + k as i16 * 13;
            let have = s.available(i, k as u8);
            let focused = usize::from(cur) == k;
            if focused {
                let a = g.c(Role::Accent);
                g.fill_round(Rect::new(r.x + 2, y, r.w - 4, 13), 3, a);
            }
            let role = if focused {
                Role::OnAccent
            } else if have {
                Role::Text
            } else {
                Role::TextDim
            };
            let c = g.c(role);
            g.text(r.x + 6, y + 10, l.get(*name), FontId::Ui, c);
            let mut v = StrBuf::<8>::new();
            if k == 0 {
                v.push('∞');
            } else {
                v.push_u32(u32::from(t.rounds(k as u8)));
            }
            g.text_in(r.x + 2, y + 10, r.w - 8, v.as_str(), FontId::Ui, c, Align::Right);
        }
    }

    fn draw_shop(&self, g: &mut Gfx<'_>, l: Lang) {
        let s = self.s;
        let i = usize::from(s.turn());
        let t = s.tanks()[i];
        let r = Rect::new(2, HUD_H + 1, 124, W - HUD_H - 3);
        let surface = g.c(Role::Surface);
        let edge = g.pal.mix(Role::Surface, Role::TextDim, 110);
        g.fill_round(r, 4, surface);
        g.stroke_round(r, 4, edge);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let mut v = StrBuf::<32>::new();
        v.push_str(l.get(Str::GameShop));
        v.push_str(" · P");
        v.push_u32(i as u32 + 1);
        g.text(r.x + 5, r.y + 11, v.as_str(), FontId::UiBold, player_color(g, i));
        v.clear();
        push_points(&mut v, u32::from(t.credits));
        g.text_in(r.x + 2, r.y + 11, r.w - 7, v.as_str(), FontId::Ui, text, Align::Right);
        // Rows: the weapon, how many there are, the price; then Done.
        for k in 1..=WEAPONS {
            let y = r.y + 14 + (k as i16 - 1) * 12;
            let row = if k < WEAPONS { k } else { 0 };
            let focused = usize::from(s.shop_row()) == k;
            if focused {
                let a = g.c(Role::Accent);
                g.fill_round(Rect::new(r.x + 2, y, r.w - 4, 12), 3, a);
            }
            let c = if focused { g.c(Role::OnAccent) } else { text };
            if row == 0 {
                g.text(r.x + 6, y + 10, l.get(Str::Done), FontId::UiBold, c);
                continue;
            }
            g.text(r.x + 6, y + 10, l.get(WEAPON_NAMES[row]), FontId::Ui, c);
            v.clear();
            v.push('×');
            v.push_u32(u32::from(t.rounds(row as u8)));
            let owned = if focused { c } else { dim };
            g.text_in(r.x + 70, y + 10, 22, v.as_str(), FontId::Ui, owned, Align::Right);
            v.clear();
            v.push_u32(u32::from(SPECS[row].price));
            let pc = if focused {
                c
            } else if t.credits >= SPECS[row].price {
                text
            } else {
                dim
            };
            g.text_in(r.x + 2, y + 10, r.w - 8, v.as_str(), FontId::Ui, pc, Align::Right);
        }
    }

    /// The end of the game: who won, everyone's points.
    fn draw_standings(&self, g: &mut Gfx<'_>, l: Lang, ended: bool) {
        let s = self.s;
        let n = s.tanks().len() as i16;
        let r = Rect::new(8, 28, 112, 40 + n * 12);
        let surface = g.c(Role::Surface);
        let edge = g.pal.mix(Role::Surface, Role::TextDim, 110);
        g.fill_round(r, 4, surface);
        g.stroke_round(r, 4, edge);
        let mut v = StrBuf::<32>::new();
        let (title, role) = match s.leader() {
            Some(p) => {
                v.push_str(l.get(Str::GamePlayer));
                v.push(' ');
                v.push_u32(u32::from(p) + 1);
                (l.get(Str::GameWinsGame), Role::Good)
            }
            None => {
                v.push_str(l.get(Str::GameTie));
                ("", Role::Text)
            }
        };
        let c = g.c(role);
        g.text_in(r.x + 2, r.y + 12, r.w - 4, v.as_str(), FontId::UiBold, c, Align::Center);
        let text = g.c(Role::Text);
        g.text_in(r.x + 2, r.y + 24, r.w - 4, title, FontId::Ui, text, Align::Center);
        for (i, t) in s.tanks().iter().enumerate() {
            let y = r.y + 36 + i as i16 * 12;
            v.clear();
            v.push('P');
            v.push_u32(i as u32 + 1);
            g.text(r.x + 10, y, v.as_str(), FontId::UiBold, player_color(g, i));
            v.clear();
            if t.score < 0 {
                v.push('−');
            }
            push_points(&mut v, u32::from(t.score.unsigned_abs()));
            g.text_in(r.x + 2, y, r.w - 12, v.as_str(), FontId::Ui, text, Align::Right);
        }
        let dim = g.c(Role::TextDim);
        let last = if ended && self.new_best {
            l.get(Str::GameNewBest)
        } else if ended {
            l.get(Str::GameOkToStart)
        } else {
            ""
        };
        g.text_in(r.x + 2, r.bottom() - 4, r.w - 4, last, FontId::Ui, dim, Align::Center);
    }
}
