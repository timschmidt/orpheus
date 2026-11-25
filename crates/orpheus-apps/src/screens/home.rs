//! Home (05 §6.2): a 4 × 3 grid of 32 × 32 cells with 24 × 24 icons; the
//! focused cell is an accent rounded square. The strip names the focused
//! app, or shows the mini-player (the name then appears as a 1 s toast).
//! More than twelve apps (Books, M15, with Games) make a fourth row: the
//! grid scrolls by rows to keep the focused cell in view.

use orpheus_ui::event::{Event, Key, KeyPhase, UiEvent};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::theme::Role;

use super::{FILES, HOME_APPS, MUSIC, NOTICE, Notice, PICTURES, SETTINGS};
use crate::App;
use crate::services::{CardState, Services, mini_player};
use crate::settings::id;
use crate::strings::{Lang, Str};

const COLS: usize = 4;
/// Cells on the screen (three rows).
const CELLS: usize = 12;
const CELL: i16 = 32;

#[derive(Default)]
pub struct HomeScreen {
    apps: [u8; App::COUNT],
    n: u8,
    focus: u8,
    drawn_focus: Option<u8>,
    /// The first row drawn (more than twelve apps).
    drawn_row: u8,
    lang: Lang,
}

impl HomeScreen {
    fn load<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        let mask = svc.setting(id::HOME_APPS);
        self.n = 0;
        for app in App::GRID {
            let shown = app == App::Settings || mask & (1 << app as u8) != 0;
            if shown && svc.app_available(app) {
                self.apps[usize::from(self.n)] = app as u8;
                self.n += 1;
            }
        }
        self.focus = self.focus.min(self.n.saturating_sub(1));
    }

    fn app(&self, pos: u8) -> Option<App> {
        if pos >= self.n {
            return None;
        }
        self.apps.get(usize::from(pos)).and_then(|&i| App::from_index(i))
    }

    /// Cells of the whole grid: twelve, or whole rows for more apps (only
    /// Books makes more than twelve: without it, the constant, so the
    /// firmware's Home code is what it was).
    fn cells(&self) -> usize {
        #[cfg(feature = "books")]
        {
            usize::from(self.n).div_ceil(COLS).max(3) * COLS
        }
        #[cfg(not(feature = "books"))]
        {
            CELLS
        }
    }

    /// The first row shown: the focused cell in view.
    fn top_row(&self) -> u8 {
        #[cfg(feature = "books")]
        {
            (self.focus / COLS as u8).saturating_sub(2)
        }
        #[cfg(not(feature = "books"))]
        {
            0
        }
    }

    fn step(&self, k: Key) -> u8 {
        let n = usize::from(self.n.max(1));
        let p = usize::from(self.focus);
        let cells = self.cells();
        let next = match k {
            Key::Next => (p + 1) % n,
            Key::Prev => (p + n - 1) % n,
            Key::Down | Key::Up => {
                // Move by a row, wrapping within the column; empty cells
                // are skipped.
                let d = if k == Key::Down { COLS } else { cells - COLS };
                let mut c = p;
                for _ in 0..cells / COLS {
                    c = (c + d) % cells;
                    if c < n {
                        break;
                    }
                }
                if c < n { c } else { p }
            }
            _ => p,
        };
        u8::try_from(next).unwrap_or(0)
    }

    fn cell(&self, pos: u8) -> Rect {
        let p = i16::from(pos) - i16::from(self.top_row()) * COLS as i16;
        Rect::new((p % 4) * CELL, (p / 4) * CELL, CELL, CELL)
    }

    fn draw_cell(&self, g: &mut Gfx<'_>, pos: u8) {
        let r = self.cell(pos);
        g.fill_role(r, Role::Bg);
        let Some(app) = self.app(pos) else { return };
        let focused = pos == self.focus;
        let fg = if focused {
            let acc = g.c(Role::Accent);
            g.fill_round(r.inset(1, 1), 8, acc);
            g.c(Role::OnAccent)
        } else {
            g.c(Role::Text)
        };
        g.icon2(r.x + 4, r.y + 4, app.icon(), fg);
    }
}

impl<S: Services + ?Sized> Screen<S> for HomeScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.load(cx.svc);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(
                k @ (Key::Up | Key::Down | Key::Prev | Key::Next),
                KeyPhase::Down | KeyPhase::Repeat,
            ) => {
                let f = self.step(k);
                if f != self.focus {
                    self.focus = f;
                    if mini_player(cx.svc).is_some()
                        && let Some(app) = self.app(f)
                    {
                        cx.ui.toast_for(self.lang.get(app.label()), 1000);
                    }
                }
                Nav::Stay
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                let Some(app) = self.app(self.focus) else { return Nav::Stay };
                if app == App::Settings {
                    return Nav::Push(SETTINGS, Arg(0));
                }
                if app == App::Bluetooth {
                    return Nav::Push(super::BLUETOOTH, Arg(0));
                }
                if app == App::Radio {
                    return Nav::Push(super::RADIO, Arg(0));
                }
                if app == App::Steps {
                    return Nav::Push(super::STEPS, Arg(0));
                }
                if app == App::Stopwatch {
                    return Nav::Push(super::STOPWATCH, Arg(0));
                }
                if app == App::Authenticator {
                    return Nav::Push(super::AUTHENTICATOR, Arg(0));
                }
                #[cfg(feature = "games")]
                if app == App::Games {
                    return Nav::Push(super::GAMES, Arg(0));
                }
                #[cfg(feature = "books")]
                if app == App::Books {
                    return match cx.svc.card() {
                        CardState::Ready => Nav::Push(super::BOOKS, Arg(0)),
                        CardState::Unreadable => Nav::Push(NOTICE, Arg(Notice::CardUnreadable.arg())),
                        _ => Nav::Push(NOTICE, Arg(Notice::NoCard.arg())),
                    };
                }
                if app == App::Recorder && cx.svc.card() == CardState::Ready {
                    return Nav::Push(super::RECORDER, Arg(0));
                }
                let notice = if app.needs_card() {
                    match cx.svc.card() {
                        CardState::Absent => Notice::NoCard,
                        CardState::Unreadable => Notice::CardUnreadable,
                        CardState::UsbOwned if app != App::UsbDrive => Notice::NoCard,
                        _ => match app {
                            App::Music => return Nav::Push(MUSIC, Arg(0)),
                            App::Files => return Nav::Push(FILES, Arg(0)),
                            App::Pictures => return Nav::Push(PICTURES, Arg(0)),
                            // Also while the card is lent: the export's status.
                            App::UsbDrive => return Nav::Push(super::USB_DRIVE, Arg(0)),
                            _ => Notice::NotYet(app),
                        },
                    }
                } else {
                    Notice::NotYet(app)
                };
                Nav::Push(NOTICE, Arg(notice.arg()))
            }
            Event::Ui(UiEvent::Menu { id: 1, .. }) => Nav::Push(HOME_APPS, Arg(0)),
            Event::Service(_) => {
                let before = (self.n, self.lang.code());
                self.load(cx.svc);
                if before != (self.n, self.lang.code()) {
                    self.drawn_focus = None;
                    cx.ui.redraw();
                }
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let row = self.top_row();
        match (dirty, self.drawn_focus) {
            (Dirty::Changed, Some(old)) if old == self.focus => {}
            (Dirty::Changed, Some(old)) if row == self.drawn_row => {
                self.draw_cell(g, old);
                self.draw_cell(g, self.focus);
            }
            _ => {
                g.clear(Role::Bg);
                let first = row * COLS as u8;
                for pos in first..first + u8::try_from(CELLS).unwrap_or(12) {
                    self.draw_cell(g, pos);
                }
            }
        }
        self.drawn_focus = Some(self.focus);
        self.drawn_row = row;
    }

    fn menu(&self, _cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_none() {
            out.action(1, self.lang.get(Str::HomeScreen));
        }
    }

    fn wants(&self) -> Wants {
        let name = self.app(self.focus).map_or("", |a| self.lang.get(a.label()));
        Wants { strip: StripUse::Hint(name), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: u32::from(self.focus), ..Resume::default() }
    }

    fn restore(&mut self, _cx: &mut Cx<'_, S>, r: &Resume) {
        self.focus = u8::try_from(r.focus).unwrap_or(0).min(self.n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(n: u8) -> HomeScreen {
        let mut h = HomeScreen { n, ..HomeScreen::default() };
        for i in 0..n {
            h.apps[usize::from(i)] = i;
        }
        h
    }

    #[test]
    fn grid_navigation_skips_empty_cell() {
        let mut h = home(11);
        h.focus = 7;
        assert_eq!(h.step(Key::Down), 3, "cell 11 is empty: wrap to the top");
        h.focus = 3;
        assert_eq!(h.step(Key::Up), 7);
        h.focus = 10;
        assert_eq!(h.step(Key::Next), 0);
        h.focus = 0;
        assert_eq!(h.step(Key::Prev), 10);
        assert_eq!(h.step(Key::Up), 8);
        let mut small = home(5);
        small.focus = 2;
        assert_eq!(small.step(Key::Down), 2, "nothing below or above: stay");
    }

    /// Thirteen apps (Books with Games): a fourth row, the grid scrolls to
    /// keep the focus in view; twelve or fewer never scroll.
    #[cfg(all(feature = "games", feature = "books"))]
    #[test]
    fn a_fourth_row_scrolls() {
        let mut h = home(13);
        h.focus = 11;
        assert_eq!(h.top_row(), 0);
        assert_eq!(h.step(Key::Next), 12);
        h.focus = 12;
        assert_eq!(h.top_row(), 1);
        assert_eq!(h.cell(12), Rect::new(0, 64, CELL, CELL));
        assert_eq!(h.step(Key::Down), 0, "wraps to the top of the column");
        assert_eq!(h.step(Key::Up), 8);
        h.focus = 9;
        assert_eq!(h.step(Key::Down), 1, "cell 13 is empty: wraps");
        let full = home(12);
        assert_eq!((full.cells(), full.top_row()), (12, 0));
    }
}
