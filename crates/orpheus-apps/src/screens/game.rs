//! A game on screen (05 §7.13, M14): Blocks, 4096 and Solitaire.
//!
//! The game itself is in the service (`GamesService::games`); this screen
//! maps keys to game inputs, runs the fixed logic steps from its ticks
//! (Blocks: up to 30 frames/s, only while playing), and draws. It is
//! rebuilt whenever it comes back to the top: from the pause menu's Resume
//! or New game it plays on at once; after anything else (the lock screen,
//! an alarm, a notice), after a tick gap or with the screen off it comes
//! back paused ("OK to continue").
//!
//! Blocks is full screen: the well (10 × 20 cells of 6 px) on the left,
//! next pieces, hold, score, level, lines and the mode's clock or best on
//! the right. Keys: Prev/Next move, Up turns clockwise, OK anticlockwise,
//! Down soft drop, hold Down hard drop, Menu hold, Back pause.
//!
//! 4096 shows the status bar and uses the strip: score and best over a
//! square board of 98 px. Up/Down/Prev/Next slide (with a 4-frame slide
//! animation unless turned off), OK takes back a slide, Back pauses.
//!
//! Solitaire is full screen (`super::solitaire`): Prev/Next move between
//! the piles, Up/Down choose depth, OK picks up and puts down, Menu sends
//! cards to the foundations, hold OK undoes, Back puts held cards back or
//! pauses.
//!
//! Salvo (feature `salvo`) is full screen (`super::salvo`): Up/Down the
//! barrel, Prev/Next the power, OK fires, hold OK the overview, Menu the
//! weapon list (Back closes it), Back pauses; a shell in the air lands at
//! once when the game pauses. A computer's search runs one simulated shot
//! a frame.
//!
//! A save file a firmware's card task reads later (`GamesService`'s
//! `Open` answered late, the hub's `opening`): the screen shows "Loading…"
//! until the hub has the game.

use orpheus_games::blocks::{self, ATTACK_STEPS, Blocks, HIDDEN, LINES_GOAL, Piece, ROWS, W};
use orpheus_games::tiles::{self, Moved, Tiles, TilesOut};
use orpheus_games::{Advance, Button, Input, STEP_HZ, Session, StepClock, solitaire::SolOut};
use orpheus_ui::color::{Color, blend, channels};
use orpheus_ui::event::{Event, Instant, Key, KeyPhase};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;

#[cfg(feature = "salvo")]
use super::GAME_SALVO;
use super::games::{push_points, push_time};
#[cfg(feature = "salvo")]
use super::salvo::SalvoView;
use super::solitaire::SolitaireView;
use super::{GAME_BLOCKS, GAME_PAUSE, GAME_SOLITAIRE, GAME_TILES};
use crate::games::{self, Game, GamesRequest};
use crate::services::Services;
use crate::services::other;
use crate::strings::{Lang, Str};
use orpheus_ui::event::ServiceEvent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(super) enum View {
    Playing,
    /// Waiting for OK (a new game, or after an interruption).
    Paused,
    /// The game ended; OK starts a new one.
    Ended,
    /// The save file is being read (the hub's `opening`).
    Loading,
}

/// Frames of 4096's slide animation (one per tick, about 33 ms each).
const ANIM_FRAMES: u8 = 4;
/// Most logic steps run for one tick (a late tick does not race).
const MAX_STEPS_PER_TICK: u32 = 8;

pub struct GameScreen {
    game: Game,
    view: View,
    clock: StepClock,
    lang: Lang,
    new_best: bool,
    /// 4096: the slide being animated and its frame.
    anim: Option<(Moved, u8)>,
    /// What was drawn: the whole, the well (Blocks), the panel (Blocks).
    drawn: Option<(u32, u32, u32)>,
    /// Solitaire: what each pile and the info line looked like when drawn.
    sol_drawn: Option<[u32; 15]>,
    /// The cards held could not go there: the target flashes.
    refused: u8,
    /// The OK press was the screen's (resume, new game): its release is
    /// not a game input.
    eat_release: bool,
}

impl GameScreen {
    #[must_use]
    pub fn new(game: Game) -> GameScreen {
        GameScreen {
            game,
            view: View::Paused,
            clock: StepClock::new(),
            lang: Lang::ENGLISH,
            new_best: false,
            anim: None,
            drawn: None,
            sol_drawn: None,
            refused: 0,
            eat_release: false,
        }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.game {
            Game::Blocks => GAME_BLOCKS,
            Game::Tiles => GAME_TILES,
            Game::Solitaire => GAME_SOLITAIRE,
            #[cfg(feature = "salvo")]
            Game::Salvo => GAME_SALVO,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.game {
            Game::Blocks => "Blocks",
            Game::Tiles => "Game4096",
            Game::Solitaire => "Solitaire",
            #[cfg(feature = "salvo")]
            Game::Salvo => "Salvo",
        }
    }

    /// Real-time games run logic steps from the ticks (Solitaire: its
    /// timer and auto-complete).
    fn stepped(&self) -> bool {
        match self.game {
            Game::Blocks | Game::Solitaire => true,
            Game::Tiles => false,
            #[cfg(feature = "salvo")]
            Game::Salvo => true,
        }
    }

    fn pause<S: Services + ?Sized>(&mut self, svc: &mut S) {
        if self.view == View::Playing {
            self.view = View::Paused;
        }
        self.clock.reset();
        match &mut svc.games_mut().session {
            Session::Blocks(b) => b.release_all(),
            Session::Solitaire(s) => {
                s.drop_held();
            }
            // A shot in the air lands now: a pause is saved, and a save
            // holds no flight.
            #[cfg(feature = "salvo")]
            Session::Salvo(s) => {
                s.release_all();
                s.settle_now();
            }
            _ => {}
        }
    }

    fn end<S: Services + ?Sized>(&mut self, svc: &mut S) {
        self.new_best = games::finish(svc);
        self.view = View::Ended;
        self.clock.reset();
    }

    fn fresh_view(&self) -> View {
        match self.game {
            Game::Blocks => View::Paused,
            Game::Tiles | Game::Solitaire => View::Playing,
            #[cfg(feature = "salvo")]
            Game::Salvo => View::Playing,
        }
    }

    /// The hub has this game (or none, and the save file is not being
    /// read): a new game if there is none, the handling settings, the view.
    fn ready<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        if !self.game.is(&cx.svc.games().session) {
            games::new_game(cx.svc, self.game);
        }
        let (das, arr) = games::blocks_handling(cx.svc);
        let sol = games::solitaire_config(cx.svc);
        match &mut cx.svc.games_mut().session {
            Session::Blocks(b) => {
                b.set_handling(das, arr);
                b.release_all();
            }
            Session::Solitaire(s) => {
                s.set_assists(sol.auto_move, sol.auto_complete);
                s.drop_held();
            }
            _ => {}
        }
        self.view = if cx.svc.games().session.in_progress() { self.fresh_view() } else { View::Ended };
        self.clock.reset();
        self.anim = None;
        self.drawn = None;
        self.sol_drawn = None;
    }

    fn key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, i: Input) {
        match self.view {
            View::Loading => {}
            View::Paused => {
                if i == Input::Press(Button::Ok) {
                    self.view = View::Playing;
                    self.clock.reset();
                    // Its release must not pick up a card.
                    self.eat_release = true;
                }
            }
            View::Ended => {
                if i == Input::Press(Button::Ok) {
                    games::new_game(cx.svc, self.game);
                    self.new_best = false;
                    self.anim = None;
                    self.sol_drawn = None;
                    self.view = self.fresh_view();
                    self.eat_release = true;
                }
            }
            View::Playing => {
                if i == Input::Release(Button::Ok) && core::mem::take(&mut self.eat_release) {
                    return;
                }
                let animate = games::tiles_animations(cx.svc);
                let ended = match &mut cx.svc.games_mut().session {
                    Session::Blocks(b) => b.input(i).ended,
                    Session::Tiles(t) => {
                        match t.input(i) {
                            TilesOut::Moved(m) if animate => self.anim = Some((m, 0)),
                            TilesOut::Moved(_) | TilesOut::Undone => self.anim = None,
                            _ => {}
                        }
                        t.over()
                    }
                    Session::Solitaire(s) => {
                        self.refused = if s.input(i) == SolOut::Refused { 6 } else { 0 };
                        s.ended()
                    }
                    #[cfg(feature = "salvo")]
                    Session::Salvo(s) => {
                        s.input(i);
                        s.ended()
                    }
                    Session::None => false,
                };
                if ended {
                    self.end(cx.svc);
                }
            }
        }
    }

    /// The save file arrived (or will not): the game is ready.
    fn poll_loading<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        if self.view == View::Loading && cx.svc.games().opening != Some(self.game.id()) {
            self.ready(cx);
        }
    }

    fn tick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, now: Instant) {
        if let Some((_, f)) = &mut self.anim {
            *f += 1;
            if *f >= ANIM_FRAMES {
                self.anim = None;
            }
        }
        self.refused = self.refused.saturating_sub(1);
        self.poll_loading(cx);
        if !self.stepped() || self.view != View::Playing {
            return;
        }
        if !cx.svc.display_on() {
            self.pause(cx.svc);
            return;
        }
        let n = match self.clock.advance(now.ms()) {
            Advance::Gap => {
                self.pause(cx.svc);
                return;
            }
            Advance::Steps(n) => n.min(MAX_STEPS_PER_TICK),
        };
        let mut ended = false;
        match &mut cx.svc.games_mut().session {
            Session::Blocks(b) => {
                for _ in 0..n {
                    if b.step().ended {
                        ended = true;
                        break;
                    }
                }
            }
            Session::Solitaire(s) => {
                for _ in 0..n {
                    s.step();
                }
                ended = s.ended();
            }
            #[cfg(feature = "salvo")]
            Session::Salvo(s) => {
                for _ in 0..n {
                    // A computer's search simulates a whole shot per step:
                    // one a frame, so a frame never waits for more.
                    let thinking = s.phase() == orpheus_games::salvo::Phase::Think;
                    if s.step().ended {
                        ended = true;
                        break;
                    }
                    if thinking {
                        break;
                    }
                }
            }
            _ => {}
        }
        if ended {
            self.end(cx.svc);
        }
    }
}

/// Solitaire moves the cursor on key repeats too.
fn sol_input_of(k: Key, p: KeyPhase) -> Option<Input> {
    match (k, p) {
        (Key::Up | Key::Down | Key::Prev | Key::Next, KeyPhase::Repeat) => input_of(k, KeyPhase::Down),
        _ => input_of(k, p),
    }
}

fn input_of(k: Key, p: KeyPhase) -> Option<Input> {
    let b = match k {
        Key::Up => Button::Up,
        Key::Down => Button::Down,
        Key::Prev => Button::Left,
        Key::Next => Button::Right,
        Key::Ok => Button::Ok,
        Key::Menu => Button::Menu,
        _ => return None,
    };
    match p {
        KeyPhase::Down => Some(Input::Press(b)),
        KeyPhase::Up { .. } => Some(Input::Release(b)),
        KeyPhase::Hold => Some(Input::Long(b)),
        _ => None,
    }
}

impl<S: Services + ?Sized> Screen<S> for GameScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.eat_release = false;
        self.refused = 0;
        cx.svc.games_request(GamesRequest::Open(self.game.id()));
        if cx.svc.games().opening == Some(self.game.id()) {
            self.view = View::Loading;
            self.drawn = None;
            self.sol_drawn = None;
            return;
        }
        self.ready(cx);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                // Solitaire: Back first puts held cards back; Salvo: it
                // closes the weapon list.
                if self.view == View::Playing
                    && let Session::Solitaire(s) = &mut cx.svc.games_mut().session
                    && s.drop_held()
                {
                    return Nav::Stay;
                }
                #[cfg(feature = "salvo")]
                if self.view == View::Playing
                    && let Session::Salvo(s) = &mut cx.svc.games_mut().session
                    && s.close_menu()
                {
                    return Nav::Stay;
                }
                if self.view == View::Loading {
                    return Nav::Pop;
                }
                self.pause(cx.svc);
                cx.svc.games_request(GamesRequest::Save);
                return Nav::Push(GAME_PAUSE, Arg(u32::from(self.game.id())));
            }
            Event::Key(k, p) => {
                let i = if self.game == Game::Solitaire { sol_input_of(k, p) } else { input_of(k, p) };
                if let Some(i) = i {
                    self.key(cx, i);
                }
            }
            Event::Tick(now) => self.tick(cx, now),
            Event::Service(ServiceEvent::Other { kind: other::GAMES, .. }) => self.poll_loading(cx),
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if self.view == View::Loading {
            if dirty == Dirty::All || self.drawn.is_none() {
                g.clear(Role::Bg);
                let (w, h) = g.size();
                let dim = g.c(Role::TextDim);
                g.text_in(0, h / 2 + 4, w, self.lang.get(Str::PicLoading), FontId::Ui, dim, Align::Center);
                self.drawn = Some((0, 0, 0));
            }
            return;
        }
        if !self.game.is(&cx.svc.games().session) {
            g.clear(Role::Bg);
            return;
        }
        match &cx.svc.games().session {
            Session::Blocks(b) => {
                let (ghost, previews) = games::blocks_view(cx.svc);
                let best = games::best(cx.svc, Game::Blocks);
                let v = BlocksView { b, ghost, previews, best, view: self.view, new_best: self.new_best };
                self.drawn = Some(v.draw(g, self.lang, dirty, self.drawn));
            }
            Session::Tiles(t) => {
                let best = games::best(cx.svc, Game::Tiles);
                let anim = self.anim.as_ref().map(|(m, f)| (m, *f));
                let key = tiles_key(t, anim.map(|a| a.1), best, self.view, self.new_best);
                if dirty == Dirty::Changed && self.drawn.is_some_and(|d| d.0 == key) {
                    return;
                }
                draw_tiles(g, self.lang, t, anim, best, self.view, self.new_best);
                self.drawn = Some((key, 0, 0));
            }
            Session::Solitaire(s) => {
                let v = SolitaireView {
                    s,
                    view: self.view,
                    new_best: self.new_best,
                    timer: games::solitaire_timer(cx.svc),
                    bank: games::casino_bank(cx.svc),
                    refused: self.refused > 0,
                };
                self.sol_drawn = Some(v.draw(g, self.lang, dirty, self.sol_drawn));
            }
            #[cfg(feature = "salvo")]
            Session::Salvo(s) => {
                let v = SalvoView {
                    s,
                    view: self.view,
                    guide: games::salvo_guide(cx.svc),
                    new_best: self.new_best,
                };
                let key = v.key();
                if dirty == Dirty::Changed && self.drawn.is_some_and(|d| d.0 == key) {
                    return;
                }
                v.draw(g, self.lang);
                self.drawn = Some((key, 0, 0));
            }
            Session::None => g.clear(Role::Bg),
        }
    }

    fn wants(&self) -> Wants {
        let title = Some(self.lang.get(self.game.label()));
        match self.game {
            Game::Blocks => Wants {
                ticks: matches!(self.view, View::Playing | View::Loading),
                full_screen: true,
                menu_key: true,
                title,
                ..Wants::default()
            },
            Game::Tiles => Wants {
                ticks: self.anim.is_some() || self.view == View::Loading,
                strip: StripUse::Content,
                title,
                ..Wants::default()
            },
            Game::Solitaire => Wants {
                ticks: matches!(self.view, View::Playing | View::Loading) || self.refused > 0,
                full_screen: true,
                menu_key: true,
                title,
                ..Wants::default()
            },
            #[cfg(feature = "salvo")]
            Game::Salvo => Wants {
                ticks: matches!(self.view, View::Playing | View::Loading),
                full_screen: true,
                menu_key: true,
                title,
                ..Wants::default()
            },
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: u32::from(self.view as u8), ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, _r: &Resume) {
        // The pause menu's Resume or New game: play on at once. Anything
        // else covered the game: it waits for OK.
        if core::mem::take(&mut cx.svc.games_mut().resume) && cx.svc.games().session.in_progress() {
            self.view = View::Playing;
        }
    }
}

// ------------------------------------------------------------------ drawing

pub(super) fn fnv(h: u32, b: u8) -> u32 {
    (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
}

pub(super) fn fnv_u32(h: u32, v: u32) -> u32 {
    v.to_le_bytes().iter().fold(h, |h, &b| fnv(h, b))
}

/// The colour of a piece: roles and their mixes (every theme), each piece
/// its own.
fn piece_color(g: &Gfx<'_>, p: Piece) -> Color {
    let pal = &g.pal;
    match p {
        Piece::I => pal.get(Role::Accent),
        Piece::O => pal.mix(Role::Warning, Role::Good, 128),
        Piece::T => pal.mix(Role::Accent, Role::Warning, 128),
        Piece::S => pal.get(Role::Good),
        Piece::Z => pal.get(Role::Warning),
        Piece::J => pal.mix(Role::Accent, Role::Text, 120),
        Piece::L => pal.mix(Role::Warning, Role::Text, 120),
    }
}

/// One cell: a square one pixel smaller than its pitch, its top edge lit.
fn block(g: &mut Gfx<'_>, x: i16, y: i16, size: i16, c: Color) {
    g.fill(Rect::new(x, y, size - 1, size - 1), c);
    if size > 4 {
        let lit = blend(c, g.c(Role::OnAccent), 80);
        g.hline(x, y, size - 1, lit);
    }
}

pub(super) fn banner(g: &mut Gfx<'_>, r: Rect, lines: &[(&str, FontId, Role)]) {
    let surface = g.c(Role::Surface);
    let edge = g.pal.mix(Role::Surface, Role::TextDim, 110);
    g.fill_round(r, 4, surface);
    g.stroke_round(r, 4, edge);
    let n = i16::try_from(lines.len()).unwrap_or(1);
    let pitch = 14;
    let mut y = r.y + (r.h - n * pitch) / 2 + 10;
    for &(s, font, role) in lines {
        let c = g.c(role);
        g.text_in(r.x + 4, y, r.w - 8, s, font, c, Align::Center);
        y += pitch;
    }
}

const WELL_X: i16 = 4;
const WELL_Y: i16 = 4;
const CELL: i16 = 6;
const PANEL_X: i16 = 68;
const PANEL_W: i16 = 57;

struct BlocksView<'a> {
    b: &'a Blocks,
    ghost: bool,
    previews: u8,
    best: Option<u32>,
    view: View,
    new_best: bool,
}

impl BlocksView<'_> {
    fn well_key(&self) -> u32 {
        let b = self.b;
        let mut h = 0x811C_9DC5;
        for y in HIDDEN..ROWS {
            for x in 0..W {
                h = fnv(h, b.cell(x, y));
            }
        }
        if let Some(a) = b.active() {
            for v in [a.piece as u8, a.rot, a.x as u8, a.y as u8] {
                h = fnv(h, v);
            }
        }
        let (mask, timer) = b.clearing();
        h = fnv_u32(h, if timer > 0 { mask } else { 0 });
        // The flash alternates every three steps.
        h = fnv(h, (timer / 3) & 1);
        fnv(h, u8::from(self.ghost))
    }

    fn panel_key(&self) -> u32 {
        let b = self.b;
        let mut h = 0x811C_9DC5;
        for v in [b.score(), b.lines(), u32::from(b.level()), b.steps() / STEP_HZ, self.best.unwrap_or(0)] {
            h = fnv_u32(h, v);
        }
        for k in 0..3 {
            h = fnv(h, b.next(k).map_or(9, |p| p as u8));
        }
        h = fnv(h, b.held_piece().map_or(9, |p| p as u8));
        h = fnv(h, u8::from(b.hold_ready()));
        fnv(h, self.previews)
    }

    /// Draw what changed; returns the new drawn keys.
    fn draw(
        &self,
        g: &mut Gfx<'_>,
        l: Lang,
        dirty: Dirty,
        drawn: Option<(u32, u32, u32)>,
    ) -> (u32, u32, u32) {
        let (well, panel) = (self.well_key(), self.panel_key());
        let whole = fnv_u32(fnv(fnv(0x811C_9DC5, self.view as u8), u8::from(self.new_best)), 0);
        let full = dirty == Dirty::All || drawn.is_none_or(|d| d.0 != whole) || self.view != View::Playing;
        if !full && drawn == Some((whole, well, panel)) {
            return (whole, well, panel);
        }
        if full {
            g.clear(Role::Bg);
            let frame = g.pal.mix(Role::Bg, Role::TextDim, 90);
            g.stroke(Rect::new(WELL_X - 1, WELL_Y - 1, W as i16 * CELL + 2, 20 * CELL + 2), frame);
        }
        if full || drawn.is_none_or(|d| d.1 != well) {
            self.draw_well(g);
        }
        if full || drawn.is_none_or(|d| d.2 != panel) {
            self.draw_panel(g, l);
        }
        if self.view != View::Playing {
            self.draw_banner(g, l);
        }
        (whole, well, panel)
    }

    fn draw_well(&self, g: &mut Gfx<'_>) {
        let b = self.b;
        g.fill_role(Rect::new(WELL_X, WELL_Y, W as i16 * CELL, 20 * CELL), Role::Bg);
        let dots = g.pal.mix(Role::Bg, Role::TextDim, 45);
        for y in 1..20 {
            for x in 1..W as i16 {
                g.pixel(WELL_X + x * CELL - 1, WELL_Y + y * CELL - 1, dots);
            }
        }
        let at =
            |x: i8, y: i8| (WELL_X + i16::from(x) * CELL, WELL_Y + (i16::from(y) - HIDDEN as i16) * CELL);
        let (mask, timer) = b.clearing();
        let flash = timer > 0 && (timer / 3) & 1 == 0;
        for y in HIDDEN..ROWS {
            let clearing = timer > 0 && mask & (1 << y) != 0;
            for x in 0..W {
                let c = b.cell(x, y);
                if c == 0 {
                    continue;
                }
                let (px, py) = at(x as i8, y as i8);
                let color = if clearing && flash {
                    g.c(Role::Text)
                } else {
                    piece_color(g, Piece::from_u8(c - 1).unwrap_or(Piece::I))
                };
                block(g, px, py, CELL, color);
            }
        }
        if self.ghost
            && let Some(gh) = b.ghost()
            && b.drop_distance() > 0
        {
            let c = blend(g.c(Role::Bg), piece_color(g, gh.piece), 150);
            for (x, y) in gh.cells() {
                if y as usize >= HIDDEN {
                    let (px, py) = at(x, y);
                    g.stroke(Rect::new(px, py, CELL - 1, CELL - 1), c);
                }
            }
        }
        if let Some(a) = b.active() {
            let c = piece_color(g, a.piece);
            for (x, y) in a.cells() {
                if y as usize >= HIDDEN {
                    let (px, py) = at(x, y);
                    block(g, px, py, CELL, c);
                }
            }
        }
    }

    /// A piece drawn small, centred in `r`.
    fn mini(g: &mut Gfx<'_>, r: Rect, p: Piece, size: i16, c: Color) {
        let cells = p.cells(0);
        let (x0, x1) =
            (cells.iter().map(|c| c.0).min().unwrap_or(0), cells.iter().map(|c| c.0).max().unwrap_or(0));
        let (y0, y1) =
            (cells.iter().map(|c| c.1).min().unwrap_or(0), cells.iter().map(|c| c.1).max().unwrap_or(0));
        let w = (i16::from(x1 - x0) + 1) * size;
        let h = (i16::from(y1 - y0) + 1) * size;
        let ox = r.x + (r.w - w) / 2;
        let oy = r.y + (r.h - h) / 2;
        for (x, y) in cells {
            block(g, ox + i16::from(x - x0) * size, oy + i16::from(y - y0) * size, size, c);
        }
    }

    /// Label and value on one line; the label gives way to the value.
    fn row(g: &mut Gfx<'_>, y: i16, label: &str, value: &str) {
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let vw = g.text_in(PANEL_X, y, PANEL_W, value, FontId::Ui, text, Align::Right);
        g.text_in(PANEL_X, y, PANEL_W - vw - 3, label, FontId::Ui, dim, Align::Left);
    }

    fn draw_panel(&self, g: &mut Gfx<'_>, l: Lang) {
        let b = self.b;
        g.fill_role(Rect::new(PANEL_X - 2, 0, PANEL_W + 3, 128), Role::Bg);
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        if self.previews > 0 {
            g.text_in(PANEL_X, 10, PANEL_W, l.get(Str::GameNext), FontId::Ui, dim, Align::Left);
            for k in 0..usize::from(self.previews) {
                if let Some(p) = b.next(k) {
                    let slot = Rect::new(PANEL_X + i16::try_from(k).unwrap_or(0) * 19, 13, 18, 10);
                    let c = piece_color(g, p);
                    Self::mini(g, slot, p, 4, c);
                }
            }
        }
        if b.config().hold {
            g.text_in(PANEL_X, 36, 28, l.get(Str::GameHold), FontId::Ui, dim, Align::Left);
            if let Some(p) = b.held_piece() {
                let c = piece_color(g, p);
                let c = if b.hold_ready() { c } else { blend(g.c(Role::Bg), c, 90) };
                Self::mini(g, Rect::new(PANEL_X + 30, 28, 24, 10), p, 4, c);
            }
        }
        let mut v = StrBuf::<24>::new();
        g.text_in(PANEL_X, 58, PANEL_W, l.get(Str::GameScore), FontId::Ui, dim, Align::Left);
        push_points(&mut v, b.score());
        g.text_in(PANEL_X, 72, PANEL_W, v.as_str(), FontId::UiBold, text, Align::Right);
        v.clear();
        v.push_u32(u32::from(b.level()));
        Self::row(g, 90, l.get(Str::Level), v.as_str());
        v.clear();
        v.push_u32(b.lines());
        if b.config().mode == blocks::Mode::Lines40 {
            v.push('/');
            v.push_u32(LINES_GOAL);
        }
        Self::row(g, 104, l.get(Str::GameLines), v.as_str());
        v.clear();
        let secs = |steps: u32| steps / STEP_HZ;
        match b.config().mode {
            blocks::Mode::Endless => {
                if let Some(best) = self.best {
                    push_points(&mut v, best);
                    Self::row(g, 118, l.get(Str::GameBest), v.as_str());
                }
            }
            mode => {
                let s = if mode == blocks::Mode::Attack {
                    secs(ATTACK_STEPS.saturating_sub(b.steps()) + STEP_HZ - 1)
                } else {
                    secs(b.steps())
                };
                v.push_u32(s / 60);
                v.push(':');
                v.push_u32_padded(s % 60, 2);
                Self::row(g, 118, l.get(Str::GameTime), v.as_str());
            }
        }
    }

    fn draw_banner(&self, g: &mut Gfx<'_>, l: Lang) {
        let b = self.b;
        let r = Rect::new(3, 36, 122, 58);
        match self.view {
            View::Paused => {
                let (title, sub) = if b.steps() == 0 {
                    (l.get(Str::GameBlocks), l.get(Str::GameOkToStart))
                } else {
                    (l.get(Str::GamePaused), l.get(Str::GameOkToContinue))
                };
                banner(g, r, &[(title, FontId::UiBold, Role::Text), (sub, FontId::Ui, Role::TextDim)]);
            }
            View::Ended => {
                let title = match (b.phase(), b.config().mode) {
                    (blocks::Phase::Done, blocks::Mode::Lines40) => l.get(Str::GameAllLines),
                    (blocks::Phase::Done, _) => l.get(Str::GameTimeUp),
                    _ => l.get(Str::GameOver),
                };
                let mut v = StrBuf::<24>::new();
                if b.config().mode == blocks::Mode::Lines40 && b.phase() == blocks::Phase::Done {
                    push_time(&mut v, orpheus_games::steps_to_ms(b.steps()));
                } else {
                    push_points(&mut v, b.score());
                }
                let last = if self.new_best {
                    (l.get(Str::GameNewBest), FontId::UiBold, Role::Good)
                } else {
                    (l.get(Str::GameOkNewGame), FontId::Ui, Role::TextDim)
                };
                banner(
                    g,
                    r,
                    &[(title, FontId::UiBold, Role::Text), (v.as_str(), FontId::Ui, Role::Text), last],
                );
            }
            View::Playing | View::Loading => {}
        }
    }
}

// ------------------------------------------------------------------ 4096

const BOARD: i16 = 98;
const BOARD_Y: i16 = 14;

fn tiles_key(t: &Tiles, frame: Option<u8>, best: Option<u32>, view: View, new_best: bool) -> u32 {
    let mut h = 0x811C_9DC5;
    for &e in t.cells() {
        h = fnv(h, e);
    }
    h = fnv_u32(h, t.score());
    h = fnv_u32(h, best.unwrap_or(0));
    h = fnv(h, frame.unwrap_or(0xFF));
    h = fnv(h, t.phase() as u8);
    h = fnv(h, view as u8);
    fnv(h, u8::from(new_best))
}

/// Cell side and gap for an `n × n` board.
fn geometry(n: usize) -> (i16, i16) {
    match n {
        4 => (22, 2),
        5 => (17, 2),
        _ => (14, 2),
    }
}

/// Tile colours by exponent (2, 4, … 4096): mixes of the theme's roles,
/// each step a visible change. Larger tiles go on towards the text colour.
const TILE_RAMP: [(Role, Role, u8); 12] = [
    (Role::Surface, Role::Text, 60),
    (Role::Surface, Role::Text, 120),
    (Role::Surface, Role::Accent, 150),
    (Role::Accent, Role::Accent, 0),
    (Role::Accent, Role::Good, 128),
    (Role::Good, Role::Good, 0),
    (Role::Good, Role::Warning, 100),
    (Role::Good, Role::Warning, 175),
    (Role::Warning, Role::Warning, 0),
    (Role::Warning, Role::Accent, 110),
    (Role::Accent, Role::Warning, 70),
    (Role::Good, Role::Text, 110),
];

/// A tile's colour (0: an empty cell).
fn tile_color(g: &Gfx<'_>, e: u8) -> Color {
    let p = &g.pal;
    match TILE_RAMP.get(usize::from(e).wrapping_sub(1)) {
        _ if e == 0 => p.mix(Role::Surface, Role::TextDim, 35),
        Some(&(a, b, alpha)) => p.mix(a, b, alpha),
        None => p.mix(Role::Text, Role::Good, 40u8.saturating_mul(e - 12).min(160)),
    }
}

fn luma(c: Color) -> i32 {
    let (r, g, b) = channels(c);
    // 5/6/5 bits to a 0..=255-ish weighted sum.
    (i32::from(r) * 8 * 3 + i32::from(g) * 4 * 6 + i32::from(b) * 8) / 10
}

/// The role colour with the most contrast against `bg`.
fn ink(g: &Gfx<'_>, bg: Color) -> Color {
    [Role::Text, Role::OnAccent, Role::Bg]
        .iter()
        .map(|&r| g.c(r))
        .max_by_key(|&c| (luma(c) - luma(bg)).abs())
        .unwrap_or(0)
}

/// Digits 0–9 and `K`, 3 × 5 pixels, a row per 3 bits (own drawing): for
/// values too wide for the text fonts on a 6 × 6 board.
const TINY: [[u8; 5]; 11] = [
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
    [5, 6, 4, 6, 5],
];

fn tiny_text(g: &mut Gfx<'_>, cx: i16, cy: i16, s: &str, c: Color) {
    let mut x = cx - tiny_width(s) / 2;
    for ch in s.chars() {
        let glyph = match ch {
            '0'..='9' => TINY[usize::from(ch as u8 - b'0')],
            'K' => TINY[10],
            _ => continue,
        };
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    g.pixel(x + col, cy - 2 + i16::try_from(row).unwrap_or(0), c);
                }
            }
        }
        x += 4;
    }
}

/// The value of a tile as text: in full, or in thousands ("4K", "16K")
/// when it does not fit.
fn tile_text(e: u8, short: bool, b: &mut StrBuf<12>) {
    let v: u64 = 1 << e.min(40);
    if short && v >= 1000 {
        let k = v / 1000;
        if k >= 1000 {
            b.push_u32(u32::try_from(k / 1000).unwrap_or(0));
            b.push('M');
        } else {
            b.push_u32(u32::try_from(k).unwrap_or(0));
            b.push('K');
        }
    } else {
        b.push_u32(u32::try_from(v).unwrap_or(u32::MAX));
    }
}

fn tiny_width(s: &str) -> i16 {
    i16::try_from(s.len()).unwrap_or(0) * 4 - 1
}

/// A tile with its value in the largest face that fits: the numerals, the
/// UI faces, the 3 × 5 digits; long values in thousands ("4K") when even
/// those do not fit.
fn draw_tile(g: &mut Gfx<'_>, r: Rect, e: u8) {
    let bg = tile_color(g, e);
    g.fill_round(r, 3, bg);
    if e == 0 {
        return;
    }
    let c = ink(g, bg);
    let room = r.w - 2;
    let mut s = StrBuf::<12>::new();
    for short in [false, true] {
        s.clear();
        tile_text(e, short, &mut s);
        for font in [FontId::Num16, FontId::UiBold, FontId::Ui] {
            if (font != FontId::Num16 || !short) && g.measure(s.as_str(), font) <= room {
                let lift = if font == FontId::Num16 { 16 } else { 9 };
                g.text_in(r.x, r.y + (r.h + lift) / 2, r.w, s.as_str(), font, c, Align::Center);
                return;
            }
        }
        if tiny_width(s.as_str()) <= room || short {
            // Values no 6 × 6 game reaches may run over: kept to the tile.
            g.with_clip(r, |g| tiny_text(g, r.x + r.w / 2, r.y + r.h / 2, s.as_str(), c));
            return;
        }
    }
}

fn draw_tiles(
    g: &mut Gfx<'_>,
    l: Lang,
    t: &Tiles,
    anim: Option<(&Moved, u8)>,
    best: Option<u32>,
    view: View,
    new_best: bool,
) {
    let (w, _) = g.size();
    g.clear(Role::Bg);
    let dim = g.c(Role::TextDim);
    let text = g.c(Role::Text);
    let mut v = StrBuf::<24>::new();
    push_points(&mut v, t.score());
    let x = g.text(6, 11, l.get(Str::GameScore), FontId::Ui, dim);
    g.text(x + 4, 11, v.as_str(), FontId::UiBold, text);
    if let Some(b) = best {
        v.clear();
        v.push_str(l.get(Str::GameBest));
        v.push(' ');
        push_points(&mut v, b);
        g.text_in(6, 11, w - 12, v.as_str(), FontId::Ui, dim, Align::Right);
    }
    let n = t.n();
    let (cell, gap) = geometry(n);
    let n16 = i16::try_from(n).unwrap_or(4);
    let side = n16 * cell + (n16 + 1) * gap;
    let x0 = (w - side) / 2;
    let y0 = BOARD_Y + (BOARD - side) / 2;
    let surface = g.c(Role::Surface);
    g.fill_round(Rect::new(x0, y0, side, side), 4, surface);
    let rect_of = |i: usize| {
        let (cx, cy) = (i16::try_from(i % n).unwrap_or(0), i16::try_from(i / n).unwrap_or(0));
        Rect::new(x0 + gap + cx * (cell + gap), y0 + gap + cy * (cell + gap), cell, cell)
    };
    for i in 0..n * n {
        draw_tile(g, rect_of(i), 0);
    }
    match anim {
        Some((m, f)) => {
            // The tiles on their way, with their values before the slide.
            let f = i16::from(f);
            let frames = i16::from(ANIM_FRAMES);
            for p in m.paths() {
                let (a, b) = (rect_of(usize::from(p.from)), rect_of(usize::from(p.to)));
                let r = Rect::new(a.x + (b.x - a.x) * f / frames, a.y + (b.y - a.y) * f / frames, cell, cell);
                draw_tile(g, r, p.exp);
            }
        }
        None => {
            for (i, &e) in t.cells().iter().enumerate() {
                if e != 0 {
                    draw_tile(g, rect_of(i), e);
                }
            }
        }
    }
    let r = Rect::new(4, 36, 120, 48);
    match (view, t.phase()) {
        (View::Ended, _) | (_, tiles::Phase::Over) => {
            v.clear();
            push_points(&mut v, t.score());
            let last = if new_best {
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
        (_, tiles::Phase::Won) if anim.is_none() => banner(
            g,
            Rect::new(4, 44, 120, 34),
            &[
                (l.get(Str::GameGoalReached), FontId::UiBold, Role::Good),
                (l.get(Str::GameOkKeepGoing), FontId::Ui, Role::TextDim),
            ],
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_texts() {
        let mut b = StrBuf::<12>::new();
        for (e, short, want) in [
            (1, false, "2"),
            (12, false, "4096"),
            (12, true, "4K"),
            (14, true, "16K"),
            (17, true, "131K"),
            (20, true, "1M"),
            (24, true, "16M"),
        ] {
            b.clear();
            tile_text(e, short, &mut b);
            assert_eq!(b.as_str(), want);
        }
        assert_eq!(input_of(Key::Prev, KeyPhase::Down), Some(Input::Press(Button::Left)));
        assert_eq!(input_of(Key::Down, KeyPhase::Hold), Some(Input::Long(Button::Down)));
        assert_eq!(input_of(Key::Up, KeyPhase::Repeat), None);
        assert_eq!(input_of(Key::VolUp, KeyPhase::Down), None);
    }

    #[test]
    fn boards_fit_the_area() {
        for n in 4..=6 {
            let (cell, gap) = geometry(n);
            let n = i16::try_from(n).unwrap();
            assert!(n * cell + (n + 1) * gap <= BOARD);
        }
    }
}
