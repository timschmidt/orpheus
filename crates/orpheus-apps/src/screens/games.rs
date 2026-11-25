//! The Games app's lists (05 §7.13, M14): the list of games, a game's
//! pause menu (Resume, New game, Settings, How to play, Quit) and its
//! settings, and the How to play page. The games themselves are
//! [`super::game::GameScreen`].

use orpheus_ui::event::{Event, Key, KeyPhase, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, ListAction, ListView, draw_scrollbar};

use super::rows::Rows;
use super::{CHOICE, GAME_BLOCKS, GAME_HELP, GAME_PAUSE, GAME_SETTINGS, GAME_TILES, GAMES, SLIDER};
use crate::games::{self, Game, GamesRequest};
use crate::services::Services;
use crate::settings::{Kind, SettingId, format_value};
use crate::strings::{Lang, Str};

/// Which list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameList {
    Games,
    Pause(Game),
    Settings(Game),
}

const T_RESUME: u16 = 1;
const T_NEW: u16 = 2;
const T_SETTINGS: u16 = 3;
const T_HELP: u16 = 4;
const T_QUIT: u16 = 5;
const T_BEST: u16 = 6;
const DLG_NEW: u16 = 0x500;

/// The screen a game opens in.
#[must_use]
pub const fn game_screen(g: Game) -> ScreenId {
    match g {
        Game::Blocks => GAME_BLOCKS,
        Game::Tiles => GAME_TILES,
    }
}

/// `ms` as "m:ss.t".
pub fn push_time<const N: usize>(b: &mut StrBuf<N>, ms: u32) {
    let tenths = ms / 100;
    b.push_u32(tenths / 600);
    b.push(':');
    b.push_u32_padded((tenths / 10) % 60, 2);
    b.push('.');
    b.push_u32(tenths % 10);
}

/// `n` with thin groups of three ("12 340").
pub fn push_points<const N: usize>(b: &mut StrBuf<N>, n: u32) {
    super::push_grouped(b, n);
}

/// The best result of `g` under the current settings as text.
fn push_best<S: Services + ?Sized, const N: usize>(svc: &S, g: Game, b: &mut StrBuf<N>) -> bool {
    let Some(v) = games::best(svc, g) else { return false };
    if games::best_is_time(svc, g) {
        push_time(b, v);
    } else {
        push_points(b, v);
    }
    true
}

pub struct GameListScreen {
    kind: GameList,
    list: ListView,
    rows: Rows,
    lang: Lang,
}

impl GameListScreen {
    #[must_use]
    pub fn new(kind: GameList) -> GameListScreen {
        GameListScreen { kind, list: ListView::new(), rows: Rows::default(), lang: Lang::ENGLISH }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.kind {
            GameList::Games => GAMES,
            GameList::Pause(_) => GAME_PAUSE,
            GameList::Settings(_) => GAME_SETTINGS,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            GameList::Games => "Games",
            GameList::Pause(_) => "GamePause",
            GameList::Settings(_) => "GameSettings",
        }
    }

    fn game(&self) -> Option<Game> {
        match self.kind {
            GameList::Games => None,
            GameList::Pause(g) | GameList::Settings(g) => Some(g),
        }
    }

    fn title(&self) -> &'static str {
        self.lang.get(self.game().map_or(Str::AppGames, Game::label))
    }

    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        let l = self.lang;
        let mut rows = Rows::default();
        match self.kind {
            GameList::Games => {
                if let Some(h) = rows.push(l.get(Str::AppGames), 0) {
                    h.header = true;
                }
                for g in Game::ALL {
                    let Some(r) = rows.push(l.get(g.label()), u16::from(g.id())) else { break };
                    if svc.games().can_continue(g.id()) {
                        r.value.push_str(l.get(Str::GameContinue));
                    } else {
                        push_best(svc, g, &mut r.value);
                    }
                    r.acc = Accessory::Chevron;
                }
            }
            GameList::Pause(g) => {
                let hub = svc.games();
                let playing = g.is(&hub.session) && hub.session.in_progress();
                if let Some(h) = rows.push(l.get(if playing { Str::GamePaused } else { Str::GameOver }), 0) {
                    h.header = true;
                }
                if playing {
                    rows.push(l.get(Str::GameResume), T_RESUME);
                }
                rows.push(l.get(Str::GameNew), T_NEW);
                if let Some(r) = rows.push(l.get(Str::AppSettings), T_SETTINGS) {
                    r.acc = Accessory::Chevron;
                }
                if let Some(r) = rows.push(l.get(Str::GameHowToPlay), T_HELP) {
                    r.acc = Accessory::Chevron;
                }
                let mut best = StrBuf::<24>::new();
                if push_best(svc, g, &mut best)
                    && let Some(r) = rows.push(l.get(Str::GameBest), T_BEST)
                {
                    r.value = best;
                    r.dim = true;
                }
                rows.push(l.get(Str::GameQuit), T_QUIT);
            }
            GameList::Settings(g) => {
                if let Some(h) = rows.push(l.get(Str::AppSettings), 0) {
                    h.header = true;
                }
                for s in g.settings() {
                    let v = svc.setting(s.id);
                    let Some(r) = rows.push(l.get(s.label), s.id.0) else { break };
                    match s.kind {
                        Kind::Toggle => r.acc = Accessory::Toggle(v != 0),
                        _ => {
                            format_value(s, v, l, &mut r.value);
                            r.acc = Accessory::Chevron;
                        }
                    }
                }
            }
        }
        self.rows = rows;
    }

    fn focused_tag(&self) -> Option<u16> {
        self.rows.get(self.list.focus).filter(|r| !r.header).map(|r| r.tag)
    }

    /// Back to the game, playing.
    fn resume<S: Services + ?Sized>(cx: &mut Cx<'_, S>) -> Nav {
        cx.svc.games_mut().resume = true;
        Nav::Pop
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> Nav {
        let Some(tag) = self.focused_tag() else { return Nav::Stay };
        let l = self.lang;
        match self.kind {
            GameList::Games => match Game::from_id(u8::try_from(tag).unwrap_or(0)) {
                Some(g) => {
                    // The game in progress (in RAM or saved), else a new one.
                    cx.svc.games_request(GamesRequest::Open(g.id()));
                    let s = &cx.svc.games().session;
                    if !(g.is(s) && s.in_progress()) {
                        games::new_game(cx.svc, g);
                    }
                    Nav::Push(game_screen(g), Arg(0))
                }
                None => Nav::Stay,
            },
            GameList::Pause(g) => match tag {
                T_RESUME => Self::resume(cx),
                T_NEW => {
                    let hub = cx.svc.games();
                    if g.is(&hub.session) && hub.session.in_progress() {
                        cx.ui.dialog(DialogSpec::destructive(
                            DLG_NEW,
                            l.get(Str::GameNewQ),
                            l.get(Str::GameNewText),
                            l.get(Str::Cancel),
                            l.get(Str::Start),
                        ));
                        Nav::Stay
                    } else {
                        games::new_game(cx.svc, g);
                        Self::resume(cx)
                    }
                }
                T_SETTINGS => Nav::Push(GAME_SETTINGS, Arg(u32::from(g.id()))),
                T_HELP => Nav::Push(GAME_HELP, Arg(u32::from(g.id()))),
                T_QUIT => {
                    cx.svc.games_request(GamesRequest::Save);
                    Nav::PopTo(GAMES)
                }
                _ => Nav::Stay,
            },
            GameList::Settings(g) => {
                let sid = SettingId(tag);
                let Some(d) = g.settings().iter().find(|s| s.id == sid) else { return Nav::Stay };
                match d.kind {
                    Kind::Toggle => {
                        let v = d.step(cx.svc.setting(sid), 1);
                        cx.svc.set_setting(sid, v);
                        self.changed(cx, g, sid);
                        Nav::Stay
                    }
                    Kind::Range { .. } => Nav::Push(SLIDER, Arg(u32::from(tag))),
                    _ => Nav::Push(CHOICE, Arg(u32::from(tag))),
                }
            }
        }
    }

    /// A setting of `g` changed here: say when it takes effect.
    fn changed<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, g: Game, sid: SettingId) {
        self.refresh(cx.svc);
        let hub = cx.svc.games();
        if games::applies_next_game(sid) && g.is(&hub.session) && hub.session.in_progress() {
            cx.ui.toast(self.lang.get(Str::GameNextGame));
        }
    }
}

fn kind_for(kind: GameList, arg: Arg) -> GameList {
    let g = Game::from_id(u8::try_from(arg.0).unwrap_or(0)).unwrap_or(Game::Blocks);
    match kind {
        GameList::Games => GameList::Games,
        GameList::Pause(_) => GameList::Pause(g),
        GameList::Settings(_) => GameList::Settings(g),
    }
}

/// Rows visible in the content area.
const VISIBLE: u32 = 5;

impl<S: Services + ?Sized> Screen<S> for GameListScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.kind = kind_for(self.kind, arg);
        if self.kind == GameList::Games {
            cx.svc.games_request(GamesRequest::Load);
        }
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, 0, VISIBLE, cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                return match self.kind {
                    GameList::Pause(_) => Self::resume(cx),
                    _ => Nav::Pop,
                };
            }
            Event::Ui(UiEvent::Dialog { id: DLG_NEW, button: 1 }) => {
                if let GameList::Pause(g) = self.kind {
                    games::new_game(cx.svc, g);
                    return Self::resume(cx);
                }
                return Nav::Stay;
            }
            Event::Service(_) => {
                let focus = self.list.focus;
                self.refresh(cx.svc);
                let mut l = self.list;
                l.set_focus(&self.rows, focus, VISIBLE, cx.now);
                self.list = l;
                return Nav::Stay;
            }
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, VISIBLE, cx.list());
        self.list = l;
        match act {
            ListAction::Activated(_) => self.activate(cx),
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        l.draw(&self.rows, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn wants(&self) -> Wants {
        Wants { ticks: self.list.wants_ticks(), title: Some(self.title()), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, VISIBLE, cx.now);
        self.list = l;
    }
}

/// How to play: the game's rules text, wrapped, Up/Down by line and
/// Prev/Next by page.
pub struct GameHelpScreen {
    game: Game,
    top: u16,
    /// Lines of the text as last drawn (the scroll limit).
    lines: u16,
    drawn: Option<u16>,
    lang: Lang,
}

impl Default for GameHelpScreen {
    fn default() -> Self {
        GameHelpScreen { game: Game::Blocks, top: 0, lines: 0, drawn: None, lang: Lang::ENGLISH }
    }
}

/// Text lines in the content area.
const HELP_LINES: u16 = 7;
const HELP_PITCH: i16 = 13;

impl<S: Services + ?Sized> Screen<S> for GameHelpScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.game = Game::from_id(u8::try_from(arg.0).unwrap_or(0)).unwrap_or(Game::Blocks);
        self.lang = cx.svc.lang();
        self.top = 0;
        self.drawn = None;
    }

    fn event(&mut self, _cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let max = self.lines.saturating_sub(HELP_LINES);
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(
                k @ (Key::Up | Key::Down | Key::Prev | Key::Next),
                KeyPhase::Down | KeyPhase::Repeat,
            ) => {
                self.top = match k {
                    Key::Up => self.top.saturating_sub(1),
                    Key::Down => (self.top + 1).min(max),
                    Key::Prev => self.top.saturating_sub(HELP_LINES - 1),
                    _ => (self.top + HELP_LINES - 1).min(max),
                };
            }
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn == Some(self.top) {
            return;
        }
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let text = g.c(Role::Text);
        let top = i16::try_from(self.top).unwrap_or(0);
        // Paragraphs wrapped one after the other, an empty line between.
        let mut i = 0i16;
        let visible = 0..i16::try_from(HELP_LINES).unwrap_or(7);
        for (k, para) in self.game.help().iter().enumerate() {
            if k > 0 {
                i += 1;
            }
            g.wrap(self.lang.get(*para), FontId::Ui, w - 14, |g, line| {
                let row = i - top;
                if visible.contains(&row) {
                    g.text(6, 11 + row * HELP_PITCH, line, FontId::Ui, text);
                }
                i += 1;
            });
        }
        self.lines = u16::try_from(i).unwrap_or(u16::MAX);
        if self.lines > HELP_LINES {
            draw_scrollbar(
                g,
                Rect::new(w - 4, 2, 2, h - 4),
                u32::from(self.top),
                u32::from(HELP_LINES),
                u32::from(self.lines),
            );
        }
        self.drawn = Some(self.top);
    }

    fn wants(&self) -> Wants {
        Wants { title: Some(self.lang.get(Str::GameHowToPlay)), strip: StripUse::Auto, ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { top: u32::from(self.top), ..Resume::default() }
    }

    fn restore(&mut self, _cx: &mut Cx<'_, S>, r: &Resume) {
        self.top = u16::try_from(r.top).unwrap_or(0);
    }
}
