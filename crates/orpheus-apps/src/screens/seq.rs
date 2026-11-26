//! The Sequencer's screens (05 §7.16, SEQ-1, SEQ-2; feature `seq`): the
//! project list, the grid, the keys (note entry and live recording), the
//! live screen (fill while held), the song, and value lists — transport,
//! step (with its locks), track, mixer, sound, settings.
//!
//! The model is the service's (`SeqService::seq`, an
//! `orpheus_seq::Studio`): screens hold only their cursor, so any of them
//! is rebuilt from it after being covered. Edits go straight into the
//! project; the player reads it at each step (they are heard at the next
//! step). Playing goes on whatever the screen shows.
//!
//! Grid keys (05 §7.16): Prev/Next move along the steps (held: by pages),
//! Up/Down change track (the two synth tracks below the eight drum
//! tracks: the rows scroll), OK toggles the step, hold OK edits it, Menu
//! the pattern and track menu (play/stop first), Back saves and returns to
//! the list.

use orpheus_seq::model::{
    ALL_TRACKS, Chord, LockParam, MAX_RATCHET, MAX_STEPS, MICRO_MAX, PAGE, PATTERNS, PROB_LEVELS, SWING_MAX,
    SWING_MIN, Scale, TRACKS, TRANSPOSE_MAX, track_label,
};
use orpheus_seq::model::{PlayMode, Project, note_label};
use orpheus_seq::orpheus_synth::kit::FACTORY;
use orpheus_seq::orpheus_synth::presets::MELODIC;
use orpheus_seq::orpheus_synth::{Kind, Waveform};
use orpheus_seq::player::{cutoff_code, lock_value};
use orpheus_seq::scale::NOTE_NAMES;
use orpheus_seq::{Saved, Studio, TapTempo};
use orpheus_ui::event::{Event, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row};

use super::rows::Rows;
use super::{
    CHOICE, SEQ_GRID, SEQ_KEYS, SEQ_LIVE, SEQ_MIXER, SEQ_SETTINGS, SEQ_SONG, SEQ_SOUND, SEQ_STEP, SEQ_TRACK,
    SEQ_TRANSPORT,
};
use crate::seq::{
    SeqRequest, SeqResult, new_name, push_note, push_tempo, push_track, tempo_step, toggle_play,
};
use crate::services::{Services, other};
use crate::settings::{Kind as SettingKind, SettingId, format_value};
use crate::strings::{Lang, Str};

/// Rows visible in the content area.
const VISIBLE: u32 = 5;

/// "BS Bass", "BD Kick": track `t`'s label.
fn push_track_label<const N: usize>(b: &mut StrBuf<N>, p: &Project, t: usize) {
    if t < TRACKS {
        push_track(b, p.track(t).note);
    } else {
        let (s, l) = track_label(p, t);
        b.push_str(s);
        b.push(' ');
        b.push_str(l);
    }
}

/// The sound of track `t` for headers: a drum's name, a synth track's
/// preset.
fn sound_name(p: &Project, t: usize) -> &str {
    if t < TRACKS { note_label(p.track(t).note).1 } else { p.synth(t - TRACKS).name.as_str() }
}

/// Save the open project if it has changes.
fn autosave<S: Services + ?Sized>(svc: &mut S) {
    if svc.seq().dirty() {
        svc.seq_request(SeqRequest::Save);
    }
}

// ------------------------------------------------------------ the list

/// The saved projects, after a "New project" row.
struct ListSrc<'a> {
    items: &'a [Saved],
    new: &'static str,
    playing: Option<u8>,
    playing_label: &'static str,
}

impl ListSource for ListSrc<'_> {
    fn len(&self) -> u32 {
        self.items.len() as u32 + 1
    }
    fn row(&self, i: u32) -> Row<'_> {
        if i == 0 {
            return Row::Item(Item::new(self.new));
        }
        let Some(s) = self.items.get(i as usize - 1) else { return Row::Pending };
        let mut it = Item::new(s.name.as_str()).accessory(Accessory::Chevron);
        if self.playing == Some(s.no) {
            it = it.value(self.playing_label);
        }
        Row::Item(it)
    }
}

const M_DELETE: u16 = 1;
const M_SETTINGS: u16 = 2;

/// What the list waits for after a request the service answers later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Waiting {
    Open(u8),
}

/// The project list: "New project", then the saved projects.
pub struct SeqListScreen {
    list: ListView,
    lang: Lang,
    waiting: Option<Waiting>,
}

impl Default for SeqListScreen {
    fn default() -> Self {
        SeqListScreen { list: ListView::new(), lang: Lang::ENGLISH, waiting: None }
    }
}

fn src<'a>(st: &'a Studio, l: Lang) -> ListSrc<'a> {
    ListSrc {
        items: st.list.items(),
        new: l.get(Str::SeqNewProject),
        playing: st.open.filter(|_| st.seq.playing()),
        playing_label: l.get(Str::SeqPlaying),
    }
}

impl SeqListScreen {
    fn opened<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, no: u8) -> Nav {
        match cx.svc.seq_result() {
            SeqResult::Done if cx.svc.seq().open == Some(no) => {
                self.waiting = None;
                Nav::Push(SEQ_GRID, Arg(0))
            }
            SeqResult::None => {
                self.waiting = Some(Waiting::Open(no));
                Nav::Stay
            }
            _ => {
                self.waiting = None;
                cx.ui.toast(self.lang.get(Str::SeqOpenFailed));
                Nav::Stay
            }
        }
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, i: u32) -> Nav {
        if i == 0 {
            autosave(cx.svc);
            let Some(no) = cx.svc.seq().list.free_no() else {
                cx.ui.toast(self.lang.get(Str::SeqSaveFailed));
                return Nav::Stay;
            };
            crate::seq::new_project(cx.svc, new_name(no).as_str());
            cx.svc.seq_request(SeqRequest::Save);
            self.list.invalidate();
            return Nav::Push(SEQ_GRID, Arg(0));
        }
        let Some(no) = cx.svc.seq().list.items().get(i as usize - 1).map(|s| s.no) else { return Nav::Stay };
        if cx.svc.seq().open == Some(no) {
            return Nav::Push(SEQ_GRID, Arg(0));
        }
        autosave(cx.svc);
        cx.svc.seq_request(SeqRequest::Open(no));
        self.opened(cx, no)
    }
}

impl<S: Services + ?Sized> Screen<S> for SeqListScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        cx.svc.seq_request(SeqRequest::List);
        self.list.focus = 0;
        self.list.top = 0;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                // Leaving the app: stop, keep the work.
                cx.svc.seq_mut().seq.stop();
                autosave(cx.svc);
                return Nav::Pop;
            }
            Event::Service(ServiceEvent::Other { kind: other::SEQ, .. }) => {
                self.list.invalidate();
                cx.ui.redraw();
                if let Some(Waiting::Open(no)) = self.waiting {
                    return self.opened(cx, no);
                }
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id: M_DELETE, .. }) => {
                let i = self.list.focus;
                if let Some(no) =
                    (i > 0).then(|| cx.svc.seq().list.items().get(i as usize - 1)).flatten().map(|s| s.no)
                {
                    cx.svc.seq_request(SeqRequest::Delete(no));
                    cx.ui.toast(self.lang.get(Str::SeqDeleted));
                    let n = cx.svc.seq().list.items().len() as u32;
                    self.list.focus = self.list.focus.min(n);
                    self.list.top = self.list.top.min(self.list.focus);
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id: M_SETTINGS, .. }) => return Nav::Push(SEQ_SETTINGS, Arg(0)),
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&src(cx.svc.seq(), self.lang), ev, VISIBLE, cx.list());
        self.list = l;
        match act {
            ListAction::Activated(i) => self.activate(cx, i),
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        l.draw(&src(cx.svc.seq(), self.lang), g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        if self.list.focus > 0 && !cx.svc.seq().list.items().is_empty() {
            out.action(M_DELETE, self.lang.get(Str::SeqDelete));
        }
        out.action(M_SETTINGS, self.lang.get(Str::AppSettings));
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.get(Str::AppSequencer)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, _cx: &mut Cx<'_, S>, r: &Resume) {
        self.list.focus = r.focus;
        self.list.top = r.top;
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(self.lang.get(Str::AppSequencer));
        d.list(&src(cx.svc.seq(), self.lang), &self.list, "empty");
    }
}

// ------------------------------------------------------------ the grid

/// Steps across the grid (one page).
const COLS: usize = PAGE;
/// Track rows shown at once (the synth tracks scroll in below the drums).
const ROWS: usize = 8;
/// Header height.
const HEAD: i16 = 14;
/// A track row's height.
const ROW: i16 = 12;
/// A step cell's pitch.
const CELL: i16 = 7;
/// Left edge of the cells (the track labels before it).
const GRID_X: i16 = 16;
/// Top of the footer.
const FOOT: i16 = HEAD + ROW * ROWS as i16;

const M_PLAY: u16 = 1;
const M_TRANSPORT: u16 = 2;
const M_MIXER: u16 = 3;
const M_TRACK: u16 = 4;
const M_NEXT: u16 = 5;
const M_PREV: u16 = 6;
const M_COPY: u16 = 7;
const M_CLEAR: u16 = 8;
const M_SAVE: u16 = 9;
const M_SOUND: u16 = 10;
const M_KEYS: u16 = 11;
const M_LIVE: u16 = 12;
const M_SONG: u16 = 13;
const M_WAV: u16 = 14;
const M_MIDI: u16 = 15;
/// The sub-menu of the rest (a menu holds twelve items).
const M_MORE: u16 = 16;

/// What the grid shows: redrawn when it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GridKey {
    rev: u32,
    track: u8,
    step: u8,
    play: Option<(u8, u8)>,
}

/// The grid: 16 steps × 8 of the 10 tracks, the header and the page
/// footer.
#[derive(Default)]
pub struct GridScreen {
    track: u8,
    step: u8,
    drawn: Option<GridKey>,
    lang: Lang,
}

/// The playing pattern and step, if playing.
fn playhead(st: &Studio) -> Option<(u8, u8)> {
    st.seq.position().map(|p| (p.pattern, p.step))
}

/// Export the song (WAV or MIDI) and say how it went.
fn export<S: Services + ?Sized>(cx: &mut Cx<'_, S>, r: SeqRequest, l: Lang) {
    cx.svc.seq_request(r);
    let ok = cx.svc.seq_result() == SeqResult::Done;
    cx.ui.toast(l.get(if ok { Str::SeqExported } else { Str::SeqExportFailed }));
}

impl GridScreen {
    fn len(p: &Project) -> usize {
        p.pattern(p.current()).len()
    }

    fn top(&self) -> usize {
        usize::from(self.track).saturating_sub(ROWS - 1)
    }

    fn key(&self, st: &Studio) -> GridKey {
        GridKey { rev: st.project.rev(), track: self.track, step: self.step, play: playhead(st) }
    }

    fn clamp(&mut self, p: &Project) {
        let len = Self::len(p);
        if usize::from(self.step) >= len {
            self.step = (len - 1) as u8;
        }
    }

    fn move_step(&mut self, p: &Project, next: bool, page: bool) {
        let len = Self::len(p);
        let s = usize::from(self.step);
        let to = if page && len > PAGE {
            let pages = len.div_ceil(PAGE);
            let cur = s / PAGE;
            let np = if next { (cur + 1) % pages } else { (cur + pages - 1) % pages };
            np * PAGE
        } else if next {
            (s + 1) % len
        } else {
            (s + len - 1) % len
        };
        self.step = to as u8;
    }

    fn draw_all(&self, st: &Studio, g: &mut Gfx<'_>) {
        let p = &st.project;
        let pat = p.pattern(p.current());
        let len = pat.len();
        let page = usize::from(self.step) / PAGE;
        let play = playhead(st);
        let play_step = play.filter(|(pp, _)| usize::from(*pp) == p.current()).map(|(_, s)| usize::from(s));
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let accent = g.c(Role::Accent);
        let good = g.c(Role::Good);
        // Header: pattern, the track's sound, tempo and transport.
        g.fill_role(Rect::new(0, 0, 128, HEAD), Role::Surface);
        let mut b = StrBuf::<24>::new();
        b.push('P');
        b.push_u32(p.current() as u32 + 1);
        let x = g.text(2, 11, b.as_str(), FontId::UiBold, accent);
        let name = sound_name(p, usize::from(self.track));
        g.text_in(x + 5, 11, 92 - x, name, FontId::Ui, text, Align::Left);
        b.clear();
        push_tempo(&mut b, p.tempo());
        g.text_in(92, 11, 23, b.as_str(), FontId::Ui, dim, Align::Right);
        if st.seq.playing() {
            for k in 0..7 {
                g.vline(119 + k, 3 + k / 2, 9 - 2 * (k / 2), good);
            }
        } else {
            g.fill(Rect::new(119, 4, 7, 7), dim);
        }
        // Tracks.
        let top = self.top();
        for row in 0..ROWS {
            let t = top + row;
            let y = HEAD + ROW * row as i16;
            g.fill_role(Rect::new(0, y, 128, ROW), Role::Bg);
            let (_, solo, _) = p.mix(t);
            let c = if t == usize::from(self.track) {
                accent
            } else if solo {
                good
            } else if !p.audible(t) {
                g.pal.mix(Role::Bg, Role::TextDim, 110)
            } else {
                dim
            };
            // Lower case: two capitals of the UI font are wider than the
            // 16 px column.
            let mut lab = StrBuf::<4>::new();
            for ch in track_label(p, t).0.chars() {
                lab.push(ch.to_ascii_lowercase());
            }
            g.text_in(1, y + 10, 15, lab.as_str(), FontId::Ui, c, Align::Left);
            for i in 0..COLS {
                let s = page * PAGE + i;
                let x = GRID_X + CELL * i as i16;
                let cell = Rect::new(x, y + 1, CELL - 1, ROW - 2);
                if s >= len {
                    g.fill(Rect::new(x + 2, y + 5, 2, 1), g.pal.mix(Role::Bg, Role::TextDim, 70));
                    continue;
                }
                let st_ = pat.any_step(t, s);
                let here = play_step == Some(s);
                let locked = pat.locks.any(s, t);
                if !st_.on() {
                    let a = if here {
                        170
                    } else if s % 4 == 0 {
                        80
                    } else {
                        45
                    };
                    g.fill(cell, g.pal.mix(Role::Bg, Role::TextDim, a));
                    if locked {
                        g.fill(Rect::new(x + 3, y + 1, 2, 2), good);
                    }
                    continue;
                }
                let c = if st_.accent() {
                    accent
                } else if st_.velocity() < 64 {
                    g.pal.mix(Role::Bg, Role::Text, 150)
                } else {
                    text
                };
                if st_.prob() < PROB_LEVELS {
                    g.stroke(cell, c);
                } else {
                    g.fill(cell, c);
                }
                // Ratchets: the cell split by gaps (a chord plays once).
                let r = if t >= TRACKS && pat.synth_step(t - TRACKS, s).chord() != Chord::Single {
                    1
                } else {
                    i16::from(st_.ratchet())
                };
                for k in 1..r {
                    let gy = y + 1 + k * (ROW - 2) / r;
                    g.hline(x, gy, CELL - 1, g.c(Role::Bg));
                }
                if t >= TRACKS && r == 1 && pat.synth_step(t - TRACKS, s).chord() != Chord::Single {
                    // A chord: three dots.
                    for k in 0..3 {
                        g.fill(Rect::new(x + 1 + 2 * k, y + 8, 1, 1), g.c(Role::Bg));
                    }
                }
                if locked {
                    g.fill(Rect::new(x + 3, y + 1, 2, 2), good);
                }
                if here {
                    g.hline(x, y + ROW - 1, CELL - 1, good);
                }
            }
            if t == usize::from(self.track) {
                let x = GRID_X + CELL * (i16::from(self.step) % PAGE as i16);
                g.stroke(Rect::new(x - 1, y, CELL + 1, ROW), accent);
            }
        }
        // Footer: pages, the step and its velocity (and a synth step's
        // note).
        g.fill_role(Rect::new(0, FOOT, 128, 128 - FOOT), Role::Bg);
        let pages = len.div_ceil(PAGE);
        let play_page = play_step.map(|s| s / PAGE);
        for k in 0..pages {
            let r = Rect::new(2 + 9 * k as i16, FOOT + 6, 7, 5);
            if k == page {
                g.fill(r, accent);
            } else if play_page == Some(k) {
                g.fill(r, good);
            } else {
                g.stroke(r, dim);
            }
        }
        b.clear();
        b.push_u32(u32::from(self.step) + 1);
        b.push('/');
        b.push_u32(len as u32);
        let t = usize::from(self.track);
        let s = pat.any_step(t, usize::from(self.step));
        if s.on() {
            if t >= TRACKS {
                b.push_str(" · ");
                push_note(&mut b, pat.synth_step(t - TRACKS, usize::from(self.step)).pitch());
            }
            b.push_str(" · ");
            b.push_u32(u32::from(s.velocity()));
        }
        g.text_in(40, FOOT + 13, 86, b.as_str(), FontId::Ui, dim, Align::Right);
    }
}

impl<S: Services + ?Sized> Screen<S> for GridScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.drawn = None;
        self.clamp(&cx.svc.seq().project);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let p = &cx.svc.seq().project;
        self.clamp(p);
        match *ev {
            Event::Key(k @ (Key::Prev | Key::Next), ph @ (KeyPhase::Down | KeyPhase::Repeat)) => {
                self.move_step(p, k == Key::Next, ph == KeyPhase::Repeat);
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                let n = ALL_TRACKS as u8;
                self.track = if k == Key::Down { (self.track + 1) % n } else { (self.track + n - 1) % n };
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                let cur = p.current();
                cx.svc.seq_mut().project.toggle(cur, usize::from(self.track), usize::from(self.step));
            }
            Event::Key(Key::Ok, KeyPhase::Hold) => {
                return Nav::Push(SEQ_STEP, Arg((u32::from(self.track) << 8) | u32::from(self.step)));
            }
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                autosave(cx.svc);
                return Nav::Pop;
            }
            Event::Ui(UiEvent::Menu { id, .. }) => return self.menu_chosen(cx, id),
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let st = cx.svc.seq();
        let key = self.key(st);
        if dirty == Dirty::Changed && self.drawn == Some(key) {
            return;
        }
        self.draw_all(st, g);
        self.drawn = Some(key);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        if out.parent() == Some(M_MORE) {
            out.action(M_LIVE, l.get(Str::SeqLive));
            out.action(M_SONG, l.get(Str::SeqSongEdit));
            out.action(M_WAV, l.get(Str::SeqExportWav));
            out.action(M_MIDI, l.get(Str::SeqExportMidi));
            return;
        }
        if out.parent().is_some() {
            return;
        }
        let playing = cx.svc.seq().seq.playing();
        out.action(M_PLAY, l.get(if playing { Str::SeqStop } else { Str::SeqPlay }));
        out.action(M_TRANSPORT, l.get(Str::SeqTransport));
        out.action(M_MIXER, l.get(Str::SeqMixer));
        out.action(M_TRACK, l.get(Str::SeqTrack));
        out.action(M_NEXT, l.get(Str::SeqNextPattern));
        out.action(M_PREV, l.get(Str::SeqPrevPattern));
        out.action(M_COPY, l.get(Str::SeqCopyNext));
        out.action(M_CLEAR, l.get(Str::SeqClearPattern));
        out.action(M_SAVE, l.get(Str::SeqSave));
        out.action(M_SOUND, l.get(Str::SeqSound));
        out.action(M_KEYS, l.get(Str::SeqKeys));
        out.sub(M_MORE, l.get(Str::SeqMore));
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: true,
            full_screen: true,
            title: Some(self.lang.get(Str::AppSequencer)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: u32::from(self.track), top: u32::from(self.step), ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.track = (r.focus as usize).min(ALL_TRACKS - 1) as u8;
        self.step = (r.top as usize).min(MAX_STEPS - 1) as u8;
        self.clamp(&cx.svc.seq().project);
    }

    /// The pattern, tempo and transport; the focused step of its track (on
    /// or off, note and velocity) and its place in the pattern.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        use orpheus_ui::a11y::{Role, State};
        let st = cx.svc.seq();
        let p = &st.project;
        let pat = p.pattern(p.current());
        let mut t = StrBuf::<32>::new();
        t.push_str(self.lang.get(Str::SeqPattern));
        t.push(' ');
        t.push_u32(p.current() as u32 + 1);
        d.title(t.as_str());
        d.say_u32(self.lang.get(Str::SeqTempo), u32::from(p.tempo()));
        if st.seq.playing() {
            d.say(self.lang.get(Str::SeqPlaying));
        }
        let tr = usize::from(self.track);
        let s = usize::from(self.step);
        let step = pat.any_step(tr, s);
        let mut label = StrBuf::<32>::new();
        label.push_str(orpheus_seq::model::track_label(p, tr).1);
        label.push_str(", step ");
        label.push_u32(s as u32 + 1);
        let mut v = StrBuf::<24>::new();
        if step.on() {
            if tr >= TRACKS {
                push_note(&mut v, pat.synth_step(tr - TRACKS, s).pitch());
                v.push_str(" · ");
            }
            v.push_u32(u32::from(step.velocity()));
        }
        d.focus(Role::Cell, label.as_str())
            .value(v.as_str())
            .state(if step.on() { State::On } else { State::Off })
            .at(self.step.into(), pat.len() as u32);
    }
}

impl GridScreen {
    fn menu_chosen<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, id: u16) -> Nav {
        let cur = cx.svc.seq().project.current();
        let t = u32::from(self.track);
        match id {
            M_PLAY => toggle_play(cx.svc),
            M_TRANSPORT => return Nav::Push(SEQ_TRANSPORT, Arg(0)),
            M_MIXER => return Nav::Push(SEQ_MIXER, Arg(0)),
            M_TRACK => return Nav::Push(SEQ_TRACK, Arg(t)),
            M_SOUND => return Nav::Push(SEQ_SOUND, Arg(t)),
            // The keys play a synth track (the bass from a drum track).
            M_KEYS => return Nav::Push(SEQ_KEYS, Arg((t.max(TRACKS as u32) << 8) | u32::from(self.step))),
            M_LIVE => return Nav::Push(SEQ_LIVE, Arg(0)),
            M_SONG => return Nav::Push(SEQ_SONG, Arg(0)),
            M_NEXT => cx.svc.seq_mut().project.set_current((cur + 1) % PATTERNS),
            M_PREV => cx.svc.seq_mut().project.set_current((cur + PATTERNS - 1) % PATTERNS),
            M_COPY => {
                let to = (cur + 1) % PATTERNS;
                let pr = &mut cx.svc.seq_mut().project;
                pr.copy_pattern(cur, to);
                pr.set_current(to);
            }
            M_CLEAR => cx.svc.seq_mut().project.clear_pattern(cur),
            M_SAVE => {
                cx.svc.seq_request(SeqRequest::Save);
                let ok = cx.svc.seq_result() != SeqResult::Failed;
                cx.ui.toast(self.lang.get(if ok { Str::SeqSaved } else { Str::SeqSaveFailed }));
            }
            M_WAV => export(cx, SeqRequest::ExportWav, self.lang),
            M_MIDI => export(cx, SeqRequest::ExportMidi, self.lang),
            _ => {}
        }
        self.clamp(&cx.svc.seq().project);
        Nav::Stay
    }
}

// ------------------------------------------------------ the value lists

/// Which value list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeqEdit {
    /// Play/stop, tempo, swing, tap tempo, play mode, pattern, length,
    /// kits, scale, root, transpose, fill.
    Transport,
    /// The step of the grid's cursor and its locks (arg: track << 8 |
    /// step).
    Step,
    /// A track's sound, kit, level, mute, solo (arg: track).
    Track,
    /// Every track: mute (OK), solo (hold OK), level (Prev/Next).
    Mixer,
    /// A track's sound: a drum's kit entry or a synth patch (arg: track).
    Sound,
    /// The Sequencer's settings.
    Settings,
}

const T_PLAY: u16 = 1;
const T_TEMPO: u16 = 2;
const T_SWING: u16 = 3;
const T_TAP: u16 = 4;
const T_MODE: u16 = 5;
const T_PATTERN: u16 = 6;
const T_LENGTH: u16 = 7;
const T_KIT: u16 = 8;
const T_KIT_B: u16 = 9;
const T_SCALE: u16 = 10;
const T_ROOT: u16 = 11;
const T_TRANSPOSE: u16 = 12;
const T_FILL: u16 = 13;
const T_INFO: u16 = 20;
const T_ON: u16 = 21;
const T_VEL: u16 = 22;
const T_CHANCE: u16 = 23;
const T_RATCHET: u16 = 24;
const T_DELAY: u16 = 25;
const T_ACCENT: u16 = 26;
const T_NOTE: u16 = 27;
const T_GATE: u16 = 28;
const T_CHORD: u16 = 29;
const T_SOUND: u16 = 30;
const T_LEVEL: u16 = 31;
const T_MUTE: u16 = 32;
const T_SOLO: u16 = 33;
const T_TRACK_KIT: u16 = 34;
const T_MIX: u16 = 40;
const T_LOCKS: u16 = 60;
/// Lock rows: + the parameter's code.
const T_LOCK: u16 = 61;
/// Sound rows.
const S_TUNE: u16 = 70;
const S_DECAY: u16 = 71;
const S_TONE: u16 = 72;
const S_SNAP: u16 = 73;
const S_LEVEL: u16 = 74;
const S_PAN: u16 = 75;
const S_DELAY: u16 = 76;
const S_REVERB: u16 = 77;
const S_PRESET: u16 = 78;
const S_WAVE: u16 = 79;
const S_CUTOFF: u16 = 80;
const S_RES: u16 = 81;
const S_ENV: u16 = 82;
const S_ATTACK: u16 = 83;
const S_ADECAY: u16 = 84;
const S_SUSTAIN: u16 = 85;
const S_RELEASE: u16 = 86;
const S_VIBRATO: u16 = 87;
const S_GLIDE: u16 = 88;
const S_VOICES: u16 = 89;

/// The waveforms the sound screen offers for a synth's oscillators.
const WAVES: [(Waveform, &str); 4] = [
    (Waveform::Saw, "Saw"),
    (Waveform::Pulse, "Square"),
    (Waveform::Triangle, "Triangle"),
    (Waveform::Sine, "Sine"),
];

/// A lock value as text: tune in semitones, decay as a factor, cutoff in
/// Hz, the rest in per cent or seconds.
fn push_lock<const N: usize>(b: &mut StrBuf<N>, param: LockParam, v: u8) {
    match param {
        LockParam::Tune => {
            let q = i32::from(v) - 96;
            if q != 0 {
                b.push(if q < 0 { '−' } else { '+' });
            }
            b.push_u32(q.unsigned_abs() / 4);
            match q.unsigned_abs() % 4 {
                1 => b.push_str(".25"),
                2 => b.push_str(".5"),
                3 => b.push_str(".75"),
                _ => true,
            };
        }
        LockParam::Decay => {
            // ×50: "1.20×".
            let h = u32::from(v) * 2;
            b.push_u32(h / 100);
            b.push('.');
            b.push_u32_padded(h % 100, 2);
            b.push('×');
        }
        LockParam::Cutoff => push_hz(b, lock_value(param, v)),
        LockParam::Glide => {
            b.push_u32(u32::from(v) / 100);
            b.push('.');
            b.push_u32_padded(u32::from(v) % 100, 2);
            b.push_str(" s");
        }
        LockParam::Tone | LockParam::Resonance => {
            b.push_u32(u32::from(v));
            b.push_str(" %");
        }
    }
}

/// A frequency as "350 Hz" or "1.8 kHz".
fn push_hz<const N: usize>(b: &mut StrBuf<N>, hz: f32) {
    let hz = hz as u32;
    if hz < 1000 {
        b.push_u32(hz);
        b.push_str(" Hz");
    } else {
        let t = (hz + 50) / 100;
        b.push_u32(t / 10);
        if t < 100 {
            b.push('.');
            b.push_u32(t % 10);
        }
        b.push_str(" kHz");
    }
}

/// Seconds as "0.25 s".
fn push_secs<const N: usize>(b: &mut StrBuf<N>, s: f32) {
    let c = (s * 100.0 + 0.5) as u32;
    b.push_u32(c / 100);
    b.push('.');
    b.push_u32_padded(c % 100, 2);
    b.push_str(" s");
}

/// The value track `t` plays for `param` when its step is not locked.
fn lock_base(p: &Project, t: usize, param: LockParam) -> u8 {
    if t < TRACKS {
        return p.entry(t).map_or(param.range().0, |e| param.of_entry(e));
    }
    let patch = &p.synth(t - TRACKS).patch;
    match param {
        LockParam::Cutoff => cutoff_code(patch.filter.cutoff),
        LockParam::Resonance => (patch.filter.resonance * 100.0 + 0.5) as u8,
        _ => (patch.glide * 100.0 + 0.5).min(100.0) as u8,
    }
}

/// A value list over the studio.
pub struct SeqEditScreen {
    kind: SeqEdit,
    arg: u32,
    list: ListView,
    rows: Rows,
    lang: Lang,
    tap: TapTempo,
    seen: (u32, bool),
}

impl SeqEditScreen {
    #[must_use]
    pub fn new(kind: SeqEdit) -> SeqEditScreen {
        SeqEditScreen {
            kind,
            arg: 0,
            list: ListView::new(),
            rows: Rows::default(),
            lang: Lang::ENGLISH,
            tap: TapTempo::new(),
            seen: (0, false),
        }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.kind {
            SeqEdit::Transport => SEQ_TRANSPORT,
            SeqEdit::Step => SEQ_STEP,
            SeqEdit::Track => SEQ_TRACK,
            SeqEdit::Mixer => SEQ_MIXER,
            SeqEdit::Sound => SEQ_SOUND,
            SeqEdit::Settings => SEQ_SETTINGS,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            SeqEdit::Transport => "SeqTransport",
            SeqEdit::Step => "SeqStep",
            SeqEdit::Track => "SeqTrack",
            SeqEdit::Mixer => "SeqMixer",
            SeqEdit::Sound => "SeqSound",
            SeqEdit::Settings => "SeqSettings",
        }
    }

    fn track(&self) -> usize {
        let t = if self.kind == SeqEdit::Step { self.arg >> 8 } else { self.arg };
        (t as usize).min(ALL_TRACKS - 1)
    }

    fn step(&self) -> usize {
        ((self.arg & 0xFF) as usize).min(MAX_STEPS - 1)
    }

    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        let l = self.lang;
        let st = svc.seq();
        let p = &st.project;
        let mut rows = Rows::default();
        match self.kind {
            SeqEdit::Transport => {
                if let Some(r) = rows.push(l.get(Str::SeqPlay), T_PLAY) {
                    r.acc = Accessory::Toggle(st.seq.playing());
                }
                if let Some(r) = rows.push(l.get(Str::SeqTempo), T_TEMPO) {
                    push_tempo(&mut r.value, p.tempo());
                    r.value.push_str(" BPM");
                }
                if let Some(r) = rows.push(l.get(Str::SeqSwing), T_SWING) {
                    r.value.push_u32(u32::from(p.swing()));
                    r.value.push_str(" %");
                }
                if let Some(r) = rows.push(l.get(Str::SeqTapTempo), T_TAP) {
                    for _ in 0..self.tap.count() {
                        r.value.push('•');
                    }
                }
                if let Some(r) = rows.push(l.get(Str::SeqPlayMode), T_MODE) {
                    r.value.push_str(l.get(if p.mode() == PlayMode::Song {
                        Str::SeqSong
                    } else {
                        Str::SeqPattern
                    }));
                }
                if let Some(r) = rows.push(l.get(Str::SeqPattern), T_PATTERN) {
                    r.value.push_u32(p.current() as u32 + 1);
                }
                if let Some(r) = rows.push(l.get(Str::SeqLength), T_LENGTH) {
                    r.value.push_u32(p.pattern(p.current()).len() as u32);
                    r.value.push(' ');
                    r.value.push_str(l.get(Str::SeqSteps));
                }
                if let Some(r) = rows.push(l.get(Str::SeqKit), T_KIT) {
                    r.value.push_str_or_ellipsis(p.kit_name());
                }
                if let Some(r) = rows.push(l.get(Str::SeqKitB), T_KIT_B) {
                    r.value.push_str_or_ellipsis(p.kit_name_of(1));
                }
                let (scale, root) = p.scale();
                if let Some(r) = rows.push(l.get(Str::SeqScale), T_SCALE) {
                    r.value.push_str(scale.label());
                }
                if let Some(r) = rows.push(l.get(Str::SeqRoot), T_ROOT) {
                    r.value.push_str(NOTE_NAMES[usize::from(root % 12)]);
                }
                if let Some(r) = rows.push(l.get(Str::SeqTranspose), T_TRANSPOSE) {
                    let tr = p.pattern(p.current()).transpose();
                    if tr > 0 {
                        r.value.push('+');
                    }
                    r.value.push_i32(i32::from(tr));
                }
                if let Some(r) = rows.push(l.get(Str::SeqFill), T_FILL) {
                    match p.fill() {
                        Some(f) => {
                            r.value.push('P');
                            r.value.push_u32(f as u32 + 1);
                        }
                        None => {
                            r.value.push_str(l.get(Str::SeqNoFill));
                        }
                    }
                }
            }
            SeqEdit::Step => {
                let (t, s) = (self.track(), self.step());
                let pat = p.pattern(p.current());
                let step = pat.any_step(t, s);
                if let Some(r) = rows.push("", T_INFO) {
                    push_track_label(&mut r.value, p, t);
                    r.value.push_str(" · ");
                    r.value.push_str(l.get(Str::SeqStep));
                    r.value.push(' ');
                    r.value.push_u32(s as u32 + 1);
                    r.dim = true;
                }
                if let Some(r) = rows.push(l.get(Str::SeqStepOn), T_ON) {
                    r.acc = Accessory::Toggle(step.on());
                }
                if t >= TRACKS {
                    let ss = pat.synth_step(t - TRACKS, s);
                    if let Some(r) = rows.push(l.get(Str::SeqNote), T_NOTE) {
                        push_note(&mut r.value, ss.pitch());
                    }
                    if let Some(r) = rows.push(l.get(Str::SeqGate), T_GATE) {
                        // In steps: "¼", "1", "1½", "16".
                        let q = u32::from(ss.gate_quarters());
                        if q >= 4 {
                            r.value.push_u32(q / 4);
                        }
                        match q % 4 {
                            1 => r.value.push_str("¼"),
                            2 => r.value.push_str("½"),
                            3 => r.value.push_str("¾"),
                            _ => true,
                        };
                    }
                    if let Some(r) = rows.push(l.get(Str::SeqChord), T_CHORD) {
                        r.value.push_str(ss.chord().label());
                    }
                }
                if let Some(r) = rows.push(l.get(Str::SeqVelocity), T_VEL) {
                    r.value.push_u32(u32::from(step.velocity()));
                }
                if let Some(r) = rows.push(l.get(Str::SeqChance), T_CHANCE) {
                    r.value.push_u32(u32::from(step.prob_percent()));
                    r.value.push_str(" %");
                }
                if let Some(r) = rows.push(l.get(Str::SeqRatchet), T_RATCHET) {
                    r.value.push('×');
                    r.value.push_u32(u32::from(step.ratchet()));
                }
                if let Some(r) = rows.push(l.get(Str::SeqDelay), T_DELAY) {
                    r.value.push_u32(u32::from(step.micro()));
                    r.value.push_str("/24");
                }
                if let Some(r) = rows.push(l.get(Str::SeqAccent), T_ACCENT) {
                    r.acc = Accessory::Toggle(step.accent());
                }
                if let Some(h) = rows.push(l.get(Str::SeqLocks), T_LOCKS) {
                    h.header = true;
                }
                for param in LockParam::of_track(t) {
                    let label = l.get(match param {
                        LockParam::Tune => Str::SeqTune,
                        LockParam::Decay => Str::SeqDecay,
                        LockParam::Tone => Str::SeqTone,
                        LockParam::Cutoff => Str::SeqCutoff,
                        LockParam::Resonance => Str::SeqResonance,
                        LockParam::Glide => Str::SeqGlide,
                    });
                    if let Some(r) = rows.push(label, T_LOCK + u16::from(param.code())) {
                        match pat.locks.get(s, t, *param) {
                            Some(v) => push_lock(&mut r.value, *param, v),
                            None => {
                                r.value.push('—');
                                r.dim = true;
                            }
                        }
                    }
                }
            }
            SeqEdit::Track => {
                let t = self.track();
                let (mute, solo, level) = p.mix(t);
                if let Some(r) = rows.push(l.get(Str::SeqSound), T_SOUND) {
                    r.value.push_str_or_ellipsis(sound_name(p, t));
                }
                if let Some(r) = rows.push(l.get(Str::Level), T_LEVEL) {
                    r.value.push_u32(u32::from(level));
                    r.value.push_str(" %");
                }
                if let Some(r) = rows.push(l.get(Str::SeqMute), T_MUTE) {
                    r.acc = Accessory::Toggle(mute);
                }
                if let Some(r) = rows.push(l.get(Str::SeqSolo), T_SOLO) {
                    r.acc = Accessory::Toggle(solo);
                }
                if t < TRACKS
                    && let Some(r) = rows.push(l.get(Str::SeqKit), T_TRACK_KIT)
                {
                    r.value.push_str(if p.track(t).kit == 0 { "A" } else { "B" });
                    r.value.push_str(" · ");
                    r.value.push_str_or_ellipsis(p.kit_name_of(usize::from(p.track(t).kit)));
                }
            }
            SeqEdit::Mixer => {
                for t in 0..ALL_TRACKS {
                    let (mute, solo, level) = p.mix(t);
                    let label = if t < TRACKS { note_label(p.track(t).note).1 } else { track_label(p, t).1 };
                    let Some(r) = rows.push(label, T_MIX + t as u16) else { break };
                    if solo {
                        r.value.push_str(l.get(Str::SeqSolo));
                        r.value.push_str(" · ");
                    } else if mute {
                        r.value.push_str(l.get(Str::SeqMute));
                        r.value.push_str(" · ");
                    }
                    r.value.push_u32(u32::from(level));
                    r.value.push_str(" %");
                    r.dim = !p.audible(t);
                }
            }
            SeqEdit::Sound => self.sound_rows(p, &mut rows),
            SeqEdit::Settings => {
                for s in crate::seq::shown_settings() {
                    let v = svc.setting(s.id);
                    let Some(r) = rows.push(l.get(s.label), s.id.0) else { break };
                    match s.kind {
                        SettingKind::Toggle => r.acc = Accessory::Toggle(v != 0),
                        _ => {
                            format_value(s, v, l, &mut r.value);
                            r.acc = Accessory::Chevron;
                        }
                    }
                }
            }
        }
        self.rows = rows;
        self.seen = (p.rev(), st.seq.playing());
    }

    fn sound_rows(&self, p: &Project, rows: &mut Rows) {
        let l = self.lang;
        let t = self.track();
        let pct = |r: &mut super::rows::RowData, v: f32| {
            r.value.push_u32((v * 100.0 + 0.5) as u32);
            r.value.push_str(" %");
        };
        if t < TRACKS {
            if let Some(r) = rows.push("", T_INFO) {
                push_track_label(&mut r.value, p, t);
                r.value.push_str(" · ");
                r.value.push_str(p.kit_name_of(usize::from(p.track(t).kit)));
                r.dim = true;
            }
            let Some(e) = p.entry(t).copied() else { return };
            if let Some(r) = rows.push(l.get(Str::SeqTune), S_TUNE) {
                push_lock(&mut r.value, LockParam::Tune, LockParam::Tune.of_entry(&e));
            }
            if let Some(r) = rows.push(l.get(Str::SeqDecay), S_DECAY) {
                push_lock(&mut r.value, LockParam::Decay, LockParam::Decay.of_entry(&e));
            }
            if let Some(r) = rows.push(l.get(Str::SeqTone), S_TONE) {
                r.value.push_u32(u32::from(e.tone));
                r.value.push_str(" %");
            }
            if let Some(r) = rows.push(l.get(Str::SeqSnap), S_SNAP) {
                r.value.push_u32(u32::from(e.snap));
                r.value.push_str(" %");
            }
            if let Some(r) = rows.push(l.get(Str::Level), S_LEVEL) {
                r.value.push_u32(u32::from(e.level));
                r.value.push_str(" %");
            }
            if let Some(r) = rows.push(l.get(Str::SeqPan), S_PAN) {
                match e.pan {
                    0 => {
                        r.value.push('0');
                    }
                    x if x < 0 => {
                        r.value.push_str("L ");
                        r.value.push_u32(u32::from(x.unsigned_abs()));
                    }
                    x => {
                        r.value.push_str("R ");
                        r.value.push_u32(x as u32);
                    }
                }
            }
            let [d, rv] = p.drum_sends();
            if let Some(r) = rows.push(l.get(Str::SeqDelaySend), S_DELAY) {
                r.value.push_u32(u32::from(d));
                r.value.push_str(" %");
            }
            if let Some(r) = rows.push(l.get(Str::SeqReverbSend), S_REVERB) {
                r.value.push_u32(u32::from(rv));
                r.value.push_str(" %");
            }
            return;
        }
        let s = p.synth(t - TRACKS);
        let x = &s.patch;
        if let Some(r) = rows.push(l.get(Str::SeqPreset), S_PRESET) {
            r.value.push_str_or_ellipsis(s.name.as_str());
        }
        if let Kind::Subtractive(sub) = x.kind
            && let Some(r) = rows.push(l.get(Str::SeqWave), S_WAVE)
        {
            r.value.push_str(WAVES.iter().find(|(w, _)| *w == sub.osc[0].wave).map_or("Table", |w| w.1));
        }
        if let Some(r) = rows.push(l.get(Str::SeqCutoff), S_CUTOFF) {
            push_hz(&mut r.value, x.filter.cutoff);
        }
        if let Some(r) = rows.push(l.get(Str::SeqResonance), S_RES) {
            pct(r, x.filter.resonance);
        }
        if let Some(r) = rows.push(l.get(Str::SeqEnvAmount), S_ENV) {
            let v = (x.filter.env_amount * 10.0) as i32;
            if v > 0 {
                r.value.push('+');
            } else if v < 0 {
                r.value.push('−');
            }
            r.value.push_u32(v.unsigned_abs() / 10);
            r.value.push('.');
            r.value.push_u32(v.unsigned_abs() % 10);
            r.value.push_str(" oct");
        }
        if let Some(r) = rows.push(l.get(Str::SeqAttack), S_ATTACK) {
            push_secs(&mut r.value, x.amp_env.attack);
        }
        if let Some(r) = rows.push(l.get(Str::SeqDecay), S_ADECAY) {
            push_secs(&mut r.value, x.amp_env.decay);
        }
        if let Some(r) = rows.push(l.get(Str::SeqSustain), S_SUSTAIN) {
            pct(r, x.amp_env.sustain);
        }
        if let Some(r) = rows.push(l.get(Str::SeqRelease), S_RELEASE) {
            push_secs(&mut r.value, x.amp_env.release);
        }
        if let Some(r) = rows.push(l.get(Str::SeqVibrato), S_VIBRATO) {
            r.value.push_u32((x.lfo.pitch * 100.0 + 0.5) as u32);
            r.value.push_str(" ct");
        }
        if let Some(r) = rows.push(l.get(Str::SeqGlide), S_GLIDE) {
            push_secs(&mut r.value, x.glide);
        }
        if let Some(r) = rows.push(l.get(Str::SeqVoices), S_VOICES) {
            r.value.push_str(if x.mono { "1" } else { "4" });
        }
        if let Some(r) = rows.push(l.get(Str::SeqDelaySend), S_DELAY) {
            r.value.push_u32(u32::from(s.delay));
            r.value.push_str(" %");
        }
        if let Some(r) = rows.push(l.get(Str::SeqReverbSend), S_REVERB) {
            r.value.push_u32(u32::from(s.reverb));
            r.value.push_str(" %");
        }
    }

    fn focused(&self) -> u16 {
        self.rows.get(self.list.focus).map_or(0, |r| r.tag)
    }

    /// Prev/Next on the focused row; false when a lock was refused.
    fn adjust<S: Services + ?Sized>(&mut self, svc: &mut S, up: bool, fast: bool) -> bool {
        let tag = self.focused();
        if self.kind == SeqEdit::Settings {
            if let Some(d) = crate::settings::def(SettingId(tag)) {
                let v = d.step(svc.setting(d.id), if up { 1 } else { -1 });
                svc.set_setting(d.id, v);
            }
            return true;
        }
        let p = &mut svc.seq_mut().project;
        let d: i32 = if up { 1 } else { -1 };
        let cur = p.current();
        let (t, s) = (self.track(), self.step());
        let step = p.pattern(cur).any_step(t, s);
        let by = |v: u8, n: i32, lo: i32, hi: i32| (i32::from(v) + n).clamp(lo, hi) as u8;
        let byf = |v: f32, n: f32, lo: f32, hi: f32| (v + n).clamp(lo, hi);
        let st = t.saturating_sub(TRACKS);
        match tag {
            T_TEMPO => p.set_tempo(tempo_step(p.tempo(), up, fast)),
            T_SWING => p.set_swing(by(p.swing(), d, i32::from(SWING_MIN), i32::from(SWING_MAX))),
            T_MODE => p.set_mode(if p.mode() == PlayMode::Song { PlayMode::Pattern } else { PlayMode::Song }),
            T_PATTERN => p.set_current((cur as i32 + d).rem_euclid(PATTERNS as i32) as usize),
            T_LENGTH => {
                let n = p.pattern(cur).len() as i32 + if fast { 4 * d } else { d };
                p.set_len(cur, n.clamp(1, MAX_STEPS as i32) as usize);
            }
            T_KIT | T_KIT_B => {
                let k = usize::from(tag == T_KIT_B);
                let f = p
                    .factory_kit_of(k)
                    .map_or(0, |i| (i as i32 + d).rem_euclid(FACTORY.len() as i32) as usize);
                p.set_factory_kit_of(k, f);
            }
            T_SCALE => {
                let (sc, root) = p.scale();
                let n = Scale::ALL.len() as i32;
                p.set_scale(Scale::ALL[(i32::from(sc.code()) + d).rem_euclid(n) as usize], root);
            }
            T_ROOT => {
                let (sc, root) = p.scale();
                p.set_scale(sc, (i32::from(root) + d).rem_euclid(12) as u8);
            }
            T_TRANSPOSE => {
                let tr = i32::from(p.pattern(cur).transpose()) + d;
                p.set_transpose(cur, tr.clamp(-i32::from(TRANSPOSE_MAX), i32::from(TRANSPOSE_MAX)) as i8);
            }
            T_FILL => {
                // None, P1 … P16, round.
                let f = p.fill().map_or(0, |i| i as i32 + 1);
                let f = (f + d).rem_euclid(PATTERNS as i32 + 1);
                p.set_fill(if f == 0 { None } else { Some(f as usize - 1) });
            }
            T_VEL => p.set_any_step(
                cur,
                t,
                s,
                step.with_velocity(by(step.velocity(), if fast { 8 * d } else { d }, 1, 127)),
            ),
            T_CHANCE => {
                p.set_any_step(cur, t, s, step.with_prob(by(step.prob(), d, 1, i32::from(PROB_LEVELS))));
            }
            T_RATCHET => {
                p.set_any_step(
                    cur,
                    t,
                    s,
                    step.with_ratchet(by(step.ratchet(), d, 1, i32::from(MAX_RATCHET))),
                );
            }
            T_DELAY => {
                p.set_any_step(cur, t, s, step.with_micro(by(step.micro(), d, 0, i32::from(MICRO_MAX))));
            }
            T_NOTE => {
                let ss = p.pattern(cur).synth_step(st, s);
                let (sc, root) = p.scale();
                let n = if fast {
                    (i32::from(ss.pitch()) + 12 * d).clamp(0, 127) as u8
                } else {
                    sc.step(root, sc.snap(root, ss.pitch()), up)
                };
                p.set_synth_step(cur, st, s, ss.with_note(n));
            }
            T_GATE => {
                let ss = p.pattern(cur).synth_step(st, s);
                p.set_synth_step(cur, st, s, ss.with_gate(by(ss.gate(), d, 0, 15)));
            }
            T_CHORD => {
                let ss = p.pattern(cur).synth_step(st, s);
                let c = Chord::ALL[(i32::from(ss.chord().code()) + d).rem_euclid(4) as usize];
                p.set_synth_step(cur, st, s, ss.with_chord(c));
            }
            tag if (T_LOCK..T_LOCK + 6).contains(&tag) => {
                let Some(param) = LockParam::from_code((tag - T_LOCK) as u8) else { return true };
                let (lo, hi) = param.range();
                let v = p.pattern(cur).locks.get(s, t, param).unwrap_or_else(|| lock_base(p, t, param));
                let step_by = if fast { 8 * d } else { d };
                return p.set_lock(cur, s, t, param, by(v, step_by, i32::from(lo), i32::from(hi)));
            }
            T_SOUND if t < TRACKS => {
                let kit = *p.kit_of(usize::from(p.track(t).kit));
                let e = kit.entries();
                if !e.is_empty() {
                    let at = e.iter().position(|x| x.note == p.track(t).note);
                    let i = match at {
                        Some(i) => (i as i32 + d).rem_euclid(e.len() as i32) as usize,
                        None => 0,
                    };
                    p.set_note(t, e[i].note);
                }
            }
            T_SOUND | S_PRESET => {
                let i = p.preset(st).map_or(0, |i| (i as i32 + d).rem_euclid(MELODIC.len() as i32) as usize);
                p.set_preset(st, i);
            }
            T_TRACK_KIT => p.set_track_kit(t, 1 - p.track(t).kit.min(1)),
            T_LEVEL => p.set_level(t, by(p.mix(t).2, 5 * d, 0, 100)),
            tag if (T_MIX..T_MIX + ALL_TRACKS as u16).contains(&tag) => {
                let m = usize::from(tag - T_MIX);
                p.set_level(m, by(p.mix(m).2, 5 * d, 0, 100));
            }
            S_TUNE => p.edit_entry(t, |e| e.tune = (i32::from(e.tune) + 4 * d).clamp(-96, 96) as i8),
            S_DECAY => p.edit_entry(t, |e| e.decay = by(e.decay, 5 * d, 5, 200)),
            S_TONE => p.edit_entry(t, |e| e.tone = by(e.tone, 5 * d, 0, 100)),
            S_SNAP => p.edit_entry(t, |e| e.snap = by(e.snap, 5 * d, 0, 100)),
            S_LEVEL => p.edit_entry(t, |e| e.level = by(e.level, 5 * d, 0, 100)),
            S_PAN => p.edit_entry(t, |e| e.pan = (i32::from(e.pan) + 10 * d).clamp(-100, 100) as i8),
            S_DELAY | S_REVERB if t < TRACKS => {
                let [dl, rv] = p.drum_sends();
                if tag == S_DELAY {
                    p.set_drum_sends(by(dl, 5 * d, 0, 100), rv);
                } else {
                    p.set_drum_sends(dl, by(rv, 5 * d, 0, 100));
                }
            }
            S_DELAY | S_REVERB => {
                let x = p.synth(st);
                let (dl, rv) = (x.delay, x.reverb);
                if tag == S_DELAY {
                    p.set_synth_sends(st, by(dl, 5 * d, 0, 100), rv);
                } else {
                    p.set_synth_sends(st, dl, by(rv, 5 * d, 0, 100));
                }
            }
            S_WAVE => p.edit_patch(st, |x| {
                if let Kind::Subtractive(sub) = &mut x.kind {
                    let i = WAVES.iter().position(|(w, _)| *w == sub.osc[0].wave).map_or(0, |i| i as i32);
                    sub.osc[0].wave = WAVES[(i + d).rem_euclid(WAVES.len() as i32) as usize].0;
                }
            }),
            S_CUTOFF => p.edit_patch(st, |x| {
                let c = i32::from(cutoff_code(x.filter.cutoff)) + if fast { 8 * d } else { 2 * d };
                x.filter.cutoff = lock_value(LockParam::Cutoff, c.clamp(0, 127) as u8);
            }),
            S_RES => {
                p.edit_patch(st, |x| x.filter.resonance = byf(x.filter.resonance, 0.05 * d as f32, 0.0, 1.0));
            }
            S_ENV => p.edit_patch(st, |x| {
                x.filter.env_amount = byf(x.filter.env_amount, 0.5 * d as f32, -8.0, 8.0);
            }),
            S_ATTACK => {
                p.edit_patch(st, |x| x.amp_env.attack = byf(x.amp_env.attack, 0.01 * d as f32, 0.0, 2.0));
            }
            S_ADECAY => {
                p.edit_patch(st, |x| x.amp_env.decay = byf(x.amp_env.decay, 0.05 * d as f32, 0.0, 4.0));
            }
            S_SUSTAIN => {
                p.edit_patch(st, |x| x.amp_env.sustain = byf(x.amp_env.sustain, 0.05 * d as f32, 0.0, 1.0));
            }
            S_RELEASE => {
                p.edit_patch(st, |x| x.amp_env.release = byf(x.amp_env.release, 0.05 * d as f32, 0.0, 4.0));
            }
            S_VIBRATO => p.edit_patch(st, |x| x.lfo.pitch = byf(x.lfo.pitch, 0.05 * d as f32, 0.0, 1.0)),
            S_GLIDE => p.edit_patch(st, |x| x.glide = byf(x.glide, 0.01 * d as f32, 0.0, 1.0)),
            S_VOICES => p.edit_patch(st, |x| {
                x.mono = !x.mono;
                x.legato = x.mono;
            }),
            _ => {}
        }
        true
    }

    /// OK on the focused row.
    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, tag: u16) -> Nav {
        if self.kind == SeqEdit::Settings {
            if let Some(d) = crate::settings::def(SettingId(tag)) {
                if d.kind == SettingKind::Toggle {
                    let v = d.step(cx.svc.setting(d.id), 1);
                    cx.svc.set_setting(d.id, v);
                } else {
                    return Nav::Push(CHOICE, Arg(u32::from(tag)));
                }
            }
            return Nav::Stay;
        }
        let (t, s) = (self.track(), self.step());
        match tag {
            T_PLAY => toggle_play(cx.svc),
            T_TAP => {
                if let Some(tempo) = self.tap.tap(cx.now.0) {
                    cx.svc.seq_mut().project.set_tempo(tempo);
                }
            }
            T_MODE | T_KIT | T_KIT_B | T_SCALE | T_FILL | T_CHORD | T_TRACK_KIT | S_WAVE | S_VOICES
            | S_PRESET => {
                self.adjust(cx.svc, true, false);
            }
            tag if (T_LOCK..T_LOCK + 6).contains(&tag) => {
                if let Some(param) = LockParam::from_code((tag - T_LOCK) as u8) {
                    let p = &mut cx.svc.seq_mut().project;
                    let cur = p.current();
                    p.clear_lock(cur, s, t, param);
                }
            }
            _ => {
                let p = &mut cx.svc.seq_mut().project;
                let cur = p.current();
                let step = p.pattern(cur).any_step(t, s);
                match tag {
                    T_ON => p.set_any_step(cur, t, s, step.with_on(!step.on())),
                    T_ACCENT => p.set_any_step(cur, t, s, step.with_accent(!step.accent())),
                    T_MUTE => p.set_mute(t, !p.mix(t).0),
                    T_SOLO => p.set_solo(t, !p.mix(t).1),
                    tag if (T_MIX..T_MIX + ALL_TRACKS as u16).contains(&tag) => {
                        let m = usize::from(tag - T_MIX);
                        p.set_mute(m, !p.mix(m).0);
                    }
                    _ => {}
                }
            }
        }
        Nav::Stay
    }
}

impl<S: Services + ?Sized> Screen<S> for SeqEditScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        self.arg = arg.0;
        self.refresh(cx.svc);
        let mut l = self.list;
        let first = u32::from(
            matches!(self.kind, SeqEdit::Step) || (self.kind == SeqEdit::Sound && self.track() < TRACKS),
        );
        l.set_focus(&self.rows, first, VISIBLE, cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(k @ (Key::Prev | Key::Next), ph) => {
                if matches!(ph, KeyPhase::Down | KeyPhase::Repeat) {
                    if !self.adjust(cx.svc, k == Key::Next, ph == KeyPhase::Repeat) {
                        cx.ui.toast(self.lang.get(Str::SeqLocksFull));
                    }
                    self.refresh(cx.svc);
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            Event::Key(Key::Ok, KeyPhase::Hold) if self.kind == SeqEdit::Mixer => {
                let tag = self.focused();
                if (T_MIX..T_MIX + ALL_TRACKS as u16).contains(&tag) {
                    let m = usize::from(tag - T_MIX);
                    let p = &mut cx.svc.seq_mut().project;
                    p.set_solo(m, !p.mix(m).1);
                    self.refresh(cx.svc);
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            Event::Tick(_) => {
                let st = cx.svc.seq();
                if self.seen != (st.project.rev(), st.seq.playing()) {
                    self.refresh(cx.svc);
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, VISIBLE, cx.list());
        self.list = l;
        if let ListAction::Activated(i) = act
            && let Some(tag) = self.rows.get(i).map(|r| r.tag)
        {
            let nav = self.activate(cx, tag);
            self.refresh(cx.svc);
            self.list.invalidate();
            return nav;
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        l.draw(&self.rows, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn wants(&self) -> Wants {
        let title = self.lang.get(match self.kind {
            SeqEdit::Transport => Str::SeqTransport,
            SeqEdit::Step => Str::SeqStep,
            SeqEdit::Track => Str::SeqTrack,
            SeqEdit::Mixer => Str::SeqMixer,
            SeqEdit::Sound => Str::SeqSound,
            SeqEdit::Settings => Str::AppSettings,
        });
        let hint = self.lang.get(match self.kind {
            SeqEdit::Mixer => Str::SeqHintMixer,
            SeqEdit::Step if (T_LOCK..T_LOCK + 6).contains(&self.focused()) => Str::SeqHintLocks,
            _ => Str::SeqHintValues,
        });
        Wants { ticks: true, title: Some(title), strip: StripUse::Hint(hint), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, handle: self.arg, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.lang = cx.svc.lang();
        self.arg = r.handle;
        self.refresh(cx.svc);
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, VISIBLE, cx.now);
        self.list = l;
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, _cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(Screen::<S>::wants(self).title.unwrap_or(""));
        d.list(&self.rows, &self.list, "empty");
        if let Some(f) = d.focus.as_mut()
            && !f.value.is_empty()
            && f.role == orpheus_ui::a11y::Role::Item
        {
            f.role = orpheus_ui::a11y::Role::Slider;
        }
    }
}

// ------------------------------------------------------------ the song

/// The song editor: the chain of patterns with their repeats.
pub struct SongScreen {
    list: ListView,
    lang: Lang,
    seen: (u32, Option<u8>),
}

impl Default for SongScreen {
    fn default() -> Self {
        SongScreen { list: ListView::new(), lang: Lang::ENGLISH, seen: (0, None) }
    }
}

/// One song row's text: "3. P2 × 4" (and "▶" on the playing entry).
fn song_row(p: &Project, i: usize, playing: Option<u8>) -> StrBuf<24> {
    let mut b = StrBuf::new();
    let Some(e) = p.song.entries().get(i) else { return b };
    if playing == Some(i as u8) {
        b.push_str("▶ ");
    }
    b.push_u32(i as u32 + 1);
    b.push_str(".  P");
    b.push_u32(u32::from(e.pattern) + 1);
    b.push_str("  × ");
    b.push_u32(u32::from(e.repeats));
    b
}

/// The song's rows ("3.  P2  × 4", then "Add pattern"): a window of
/// entries around the list's top (`Rows` holds at most 14; a song 64).
struct SongRows<'a> {
    p: &'a Project,
    add: &'static str,
    texts: &'a [StrBuf<24>],
    first: u32,
}

impl ListSource for SongRows<'_> {
    fn len(&self) -> u32 {
        self.p.song.len() as u32 + 1
    }
    fn row(&self, i: u32) -> Row<'_> {
        if i as usize >= self.p.song.len() {
            return Row::Item(Item::new(self.add).accessory(Accessory::Chevron));
        }
        let k = i.wrapping_sub(self.first) as usize;
        match self.texts.get(k) {
            Some(t) => Row::Item(Item::new(t.as_str())),
            None => Row::Pending,
        }
    }
}

const SONG_WINDOW: usize = 8;
const M_INSERT: u16 = 1;
const M_REMOVE: u16 = 2;
const M_PLAY_SONG: u16 = 3;

impl SongScreen {
    fn playing(st: &Studio) -> Option<u8> {
        (st.project.mode() == PlayMode::Song).then(|| st.seq.position().map(|p| p.entry)).flatten()
    }

    fn with_rows<R>(&self, st: &Studio, f: impl FnOnce(&SongRows<'_>) -> R) -> R {
        let playing = Self::playing(st);
        let first = self.list.top.saturating_sub(1);
        let mut texts = [const { StrBuf::<24>::new() }; SONG_WINDOW];
        for (k, t) in texts.iter_mut().enumerate() {
            *t = song_row(&st.project, first as usize + k, playing);
        }
        let rows = SongRows { p: &st.project, add: self.lang.get(Str::SeqAddEntry), texts: &texts, first };
        f(&rows)
    }

    fn entry(&self, st: &Studio) -> Option<usize> {
        let i = self.list.focus as usize;
        (i < st.project.song.len()).then_some(i)
    }
}

impl<S: Services + ?Sized> Screen<S> for SongScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.list.focus = 0;
        self.list.top = 0;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let entry = self.entry(cx.svc.seq());
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Down | KeyPhase::Repeat) => {
                if let Some(i) = entry {
                    let d: i32 = if k == Key::Next { 1 } else { -1 };
                    cx.svc.seq_mut().project.edit_song(|s| {
                        let e = s.entries()[i];
                        let pat = (i32::from(e.pattern) + d).rem_euclid(PATTERNS as i32) as u8;
                        s.set(i, pat, e.repeats);
                    });
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            // Not the list's paging.
            Event::Key(Key::Prev | Key::Next, _) => return Nav::Stay,
            Event::Ui(UiEvent::Menu { id, .. }) => {
                let p = &mut cx.svc.seq_mut().project;
                match (id, entry) {
                    (M_INSERT, Some(i)) => {
                        let e = p.song.entries()[i];
                        if !p.song.is_full() {
                            p.edit_song(|s| {
                                s.insert(i + 1, e.pattern, e.repeats);
                            });
                        }
                    }
                    (M_REMOVE, Some(i)) => p.edit_song(|s| s.remove(i)),
                    (M_PLAY_SONG, _) => {
                        p.set_mode(PlayMode::Song);
                        cx.svc.seq_mut().seq.stop();
                        crate::seq::play(cx.svc);
                    }
                    _ => {}
                }
                let n = cx.svc.seq().project.song.len() as u32;
                self.list.focus = self.list.focus.min(n);
                self.list.invalidate();
                return Nav::Stay;
            }
            Event::Tick(_) => {
                let st = cx.svc.seq();
                let seen = (st.project.rev(), Self::playing(st));
                if seen != self.seen {
                    self.seen = seen;
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            _ => {}
        }
        let mut l = self.list;
        let act = self.with_rows(cx.svc.seq(), |r| l.handle(r, ev, VISIBLE, cx.list()));
        self.list = l;
        if let ListAction::Activated(i) = act {
            let p = &mut cx.svc.seq_mut().project;
            let cur = p.current() as u8;
            if let Some(e) = p.song.entries().get(i as usize).copied() {
                // OK: one more repeat (1 after 16).
                let rep = if e.repeats >= orpheus_seq::model::MAX_REPEATS { 1 } else { e.repeats + 1 };
                p.edit_song(|s| s.set(i as usize, e.pattern, rep));
            } else if !p.song.is_full() {
                // Add: the last entry's pattern + 1 (the current pattern
                // for the first).
                let pat = p.song.entries().last().map_or(cur, |e| (e.pattern + 1) % PATTERNS as u8);
                p.edit_song(|s| {
                    s.push(pat, 1);
                });
            }
            self.list.invalidate();
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        self.with_rows(cx.svc.seq(), |r| l.draw(r, g, dirty, cx.now, Role::Bg));
        self.list = l;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        if self.entry(cx.svc.seq()).is_some() {
            out.action(M_INSERT, l.get(Str::SeqInsertEntry));
            out.action(M_REMOVE, l.get(Str::SeqDeleteEntry));
        }
        out.action(M_PLAY_SONG, l.get(Str::SeqPlaySong));
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: true,
            title: Some(self.lang.get(Str::SeqSongEdit)),
            strip: StripUse::Hint(self.lang.get(Str::SeqHintSong)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.lang = cx.svc.lang();
        self.list.focus = r.focus;
        self.list.top = r.top;
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(self.lang.get(Str::SeqSong));
        self.with_rows(cx.svc.seq(), |rows| d.list(rows, &self.list, "empty"));
    }
}

// ------------------------------------------------------------ the keys

/// What the keys show: the project's revision, the note, the step,
/// recording, the playhead.
type KeysKey = (u32, u8, u8, bool, Option<(u8, u8)>);
/// What the live screen shows: the revision, the playhead, the fill, the
/// count-in.
type LiveKey = (u32, Option<(u8, u8)>, bool, u32);

const M_RECORD: u16 = 1;
const M_KEYS_PLAY: u16 = 2;

/// Note entry on a one-octave strip (05 §7.16 "Synth tracks"): the cursor
/// moves through the scale (Prev/Next; Up/Down an octave), OK plays the
/// note and enters it at the step cursor, which moves on (stopped or not
/// recording); while playing with Record on, OK writes it into the step
/// nearest to the press instead (quantised). Hold OK clears the step at
/// the cursor. Arg: track << 8 | step.
#[derive(Default)]
pub struct KeysScreen {
    track: u8,
    step: u8,
    note: u8,
    record: bool,
    lang: Lang,
    drawn: Option<KeysKey>,
}

impl KeysScreen {
    fn st(&self) -> usize {
        usize::from(self.track).clamp(TRACKS, ALL_TRACKS - 1) - TRACKS
    }

    fn draw_all(&self, st: &Studio, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let p = &st.project;
        let (scale, root) = p.scale();
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let accent = g.c(Role::Accent);
        g.fill_role(Rect::new(0, 0, w, 96), Role::Bg);
        // Line 1: the track and its sound, the scale.
        let mut b = StrBuf::<32>::new();
        b.push_str(orpheus_seq::model::SYNTH_LABELS[self.st()].1);
        b.push_str(" · ");
        b.push_str_or_ellipsis(p.synth(self.st()).name.as_str());
        g.text_in(4, 11, w - 8, b.as_str(), FontId::UiBold, text, Align::Left);
        b.clear();
        b.push_str(NOTE_NAMES[usize::from(root % 12)]);
        b.push(' ');
        b.push_str(scale.label());
        g.text_in(4, 23, w - 8, b.as_str(), FontId::Ui, dim, Align::Left);
        // The strip: the cursor note's octave, C to B.
        let base = self.note - self.note % 12;
        let (y0, kh) = (30i16, 40i16);
        let white = [0u8, 2, 4, 5, 7, 9, 11];
        let kw = 18i16;
        let x0 = (w - 7 * kw) / 2;
        // White keys light and black keys dark in every theme: the
        // lighter of the background and the text is the keys' white.
        let luma = |c: u16| 2 * u32::from(c >> 11) + u32::from((c >> 5) & 63) + u32::from(c & 31);
        let (lr, dr) =
            if luma(text) > luma(g.c(Role::Bg)) { (Role::Text, Role::Bg) } else { (Role::Bg, Role::Text) };
        for (k, pc) in white.iter().enumerate() {
            let n = base + pc;
            let x = x0 + kw * k as i16;
            let r = Rect::new(x, y0, kw - 1, kh);
            let c = if n == self.note {
                accent
            } else if scale.contains(root, n) {
                g.c(lr)
            } else {
                g.pal.mix(lr, dr, 110)
            };
            g.fill(r, c);
            g.stroke(r, g.pal.mix(lr, dr, 160));
        }
        for (k, pc) in [(0i16, 1u8), (1, 3), (3, 6), (4, 8), (5, 10)] {
            let n = base + pc;
            let x = x0 + kw * (k + 1) - 6;
            let r = Rect::new(x, y0, 11, kh * 3 / 5);
            let c = if n == self.note {
                accent
            } else if scale.contains(root, n) {
                g.c(dr)
            } else {
                g.pal.mix(dr, lr, 90)
            };
            g.fill(r, c);
            g.stroke(r, g.pal.mix(lr, dr, 160));
        }
        // Line 3: the note, the step, recording.
        b.clear();
        push_note(&mut b, self.note);
        g.text(4, 86, b.as_str(), FontId::UiBold, accent);
        b.clear();
        let pat = p.pattern(p.current());
        b.push_str(self.lang.get(Str::SeqStep));
        b.push(' ');
        b.push_u32(u32::from(self.step) + 1);
        b.push('/');
        b.push_u32(pat.len() as u32);
        let ss = pat.synth_step(self.st(), usize::from(self.step));
        if ss.on() {
            b.push_str(" · ");
            push_note(&mut b, ss.pitch());
        }
        g.text_in(36, 86, w - 40, b.as_str(), FontId::Ui, dim, Align::Right);
        if self.record {
            // On the scale's line, at the right: "● Recording".
            let c = g.c(Role::Warning);
            g.text_in(w - 70, 23, 66, self.lang.get(Str::SeqRecording), FontId::UiBold, c, Align::Right);
            g.fill(Rect::new(w - 78, 16, 6, 6), c);
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for KeysScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        self.track = ((arg.0 >> 8) as usize).clamp(TRACKS, ALL_TRACKS - 1) as u8;
        self.step = (arg.0 & 0xFF) as u8;
        let p = &cx.svc.seq().project;
        let ss = p.pattern(p.current()).synth_step(self.st(), usize::from(self.step));
        let (scale, root) = p.scale();
        self.note = scale.snap(root, ss.pitch());
        self.drawn = None;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let (scale, root) = cx.svc.seq().project.scale();
        let len = { cx.svc.seq().project.pattern(cx.svc.seq().project.current()).len() };
        if usize::from(self.step) >= len {
            self.step = 0;
        }
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Down | KeyPhase::Repeat) => {
                self.note = scale.step(root, scale.snap(root, self.note), k == Key::Next);
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                let n = if k == Key::Up {
                    self.note.saturating_add(12).min(127)
                } else {
                    self.note.saturating_sub(12)
                };
                self.note = scale.snap(root, n);
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                let t = usize::from(self.track);
                let st = cx.svc.seq_mut();
                let recording = self.record && st.seq.playing();
                let frame = st.seq.clock();
                st.key_note(t, self.note, 100, recording, frame);
                if !recording {
                    // Step entry: the note at the cursor, the cursor on.
                    let cur = st.project.current();
                    let s = st.project.pattern(cur).synth_step(self.st(), usize::from(self.step));
                    st.project.set_synth_step(
                        cur,
                        self.st(),
                        usize::from(self.step),
                        s.with_note(self.note).with_on(true),
                    );
                    self.step = ((usize::from(self.step) + 1) % len) as u8;
                }
            }
            Event::Key(Key::Ok, KeyPhase::Hold) => {
                // Undo: the cursor back a step, that step cleared.
                self.step = ((usize::from(self.step) + len - 1) % len) as u8;
                let st = cx.svc.seq_mut();
                let cur = st.project.current();
                let s = st.project.pattern(cur).synth_step(self.st(), usize::from(self.step));
                st.project.set_synth_step(cur, self.st(), usize::from(self.step), s.with_on(false));
            }
            Event::Ui(UiEvent::Menu { id: M_RECORD, .. }) => self.record = !self.record,
            Event::Ui(UiEvent::Menu { id: M_KEYS_PLAY, .. }) => toggle_play(cx.svc),
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let st = cx.svc.seq();
        let key = (st.project.rev(), self.note, self.step, self.record, playhead(st));
        if dirty == Dirty::Changed && self.drawn == Some(key) {
            return;
        }
        self.draw_all(st, g);
        self.drawn = Some(key);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        out.action(M_KEYS_PLAY, l.get(if cx.svc.seq().seq.playing() { Str::SeqStop } else { Str::SeqPlay }));
        out.toggle(M_RECORD, l.get(Str::SeqRecord), self.record);
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: true,
            title: Some(self.lang.get(Str::SeqKeys)),
            strip: StripUse::Hint(self.lang.get(Str::SeqHintKeys)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume {
            focus: u32::from(self.note),
            top: u32::from(self.step),
            handle: (u32::from(self.track) << 8) | u32::from(self.record),
            ..Resume::default()
        }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.lang = cx.svc.lang();
        self.note = (r.focus as u8).min(127);
        self.step = r.top as u8;
        self.track = ((r.handle >> 8) as usize).clamp(TRACKS, ALL_TRACKS - 1) as u8;
        self.record = r.handle & 1 != 0;
        self.drawn = None;
    }

    /// The synth track, its sound and scale; the note under the cursor.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let st = cx.svc.seq();
        let p = &st.project;
        d.title(self.lang.get(Str::SeqKeys));
        d.say(orpheus_seq::model::SYNTH_LABELS[self.st()].1);
        d.say(p.synth(self.st()).name.as_str());
        let (scale, root) = p.scale();
        let mut b = StrBuf::<24>::new();
        b.push_str(NOTE_NAMES[usize::from(root % 12)]);
        b.push(' ');
        b.push_str(scale.label());
        d.say(b.as_str());
        if self.record {
            d.say(self.lang.get(Str::SeqRecording));
        }
        let mut n = StrBuf::<8>::new();
        push_note(&mut n, self.note);
        let mut v = StrBuf::<16>::new();
        v.push_str("step ");
        v.push_u32(u32::from(self.step) + 1);
        d.focus(orpheus_ui::a11y::Role::Key, n.as_str()).value(v.as_str());
    }
}

// ------------------------------------------------------------ live

const M_LIVE_PLAY: u16 = 1;

/// Playing live (05 §7.16 "Playing"): OK held plays the fill pattern
/// (from the next step until let go), Prev/Next choose the pattern that
/// comes next (at the end of the playing one), Up/Down the tempo.
#[derive(Default)]
pub struct LiveScreen {
    lang: Lang,
    drawn: Option<LiveKey>,
}

impl LiveScreen {
    fn draw_all(&self, st: &Studio, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let p = &st.project;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let accent = g.c(Role::Accent);
        let good = g.c(Role::Good);
        g.fill_role(Rect::new(0, 0, w, 96), Role::Bg);
        let pos = st.seq.position();
        let playing = pos.map_or(p.current() as u8, |x| x.pattern);
        let mut b = StrBuf::<24>::new();
        b.push('P');
        b.push_u32(u32::from(playing) + 1);
        g.text_in(0, 36, w / 2, b.as_str(), FontId::Num28, text, Align::Center);
        b.clear();
        if p.mode() == PlayMode::Song {
            b.push_str(self.lang.get(Str::SeqSong));
        } else {
            b.push_str("→ P");
            b.push_u32(p.current() as u32 + 1);
        }
        g.text_in(
            w / 2,
            30,
            w / 2 - 4,
            b.as_str(),
            FontId::UiBold,
            if usize::from(playing) == p.current() { dim } else { accent },
            Align::Center,
        );
        b.clear();
        push_tempo(&mut b, p.tempo());
        b.push_str(" BPM");
        g.text_in(w / 2, 44, w / 2 - 4, b.as_str(), FontId::Ui, dim, Align::Center);
        // The playing pattern's steps, the playhead.
        let len = p.pattern(usize::from(playing)).len();
        let step = pos.map(|x| usize::from(x.step));
        let cols = len.min(PAGE);
        let cw = (w - 8) / cols as i16;
        for k in 0..cols {
            let r = Rect::new(4 + cw * k as i16, 56, cw - 2, 8);
            let here = step.is_some_and(|s| s % PAGE == k);
            if here {
                g.fill(r, good);
            } else {
                g.fill(r, g.pal.mix(Role::Bg, Role::TextDim, if k % 4 == 0 { 110 } else { 60 }));
            }
        }
        // The fill and the count-in.
        b.clear();
        let counting = st.seq.counting_in();
        if counting > 0 {
            b.push_u32(counting);
            g.text_in(0, 84, w, b.as_str(), FontId::UiBold, accent, Align::Center);
        } else {
            b.push_str(self.lang.get(Str::SeqFill));
            b.push(' ');
            match p.fill() {
                Some(f) => {
                    b.push('P');
                    b.push_u32(f as u32 + 1);
                }
                None => {
                    b.push_str(self.lang.get(Str::SeqNoFill));
                }
            }
            let held = st.seq.fill() && p.fill().is_some();
            if held {
                g.fill(Rect::new(4, 72, w - 8, 16), accent);
            }
            g.text_in(
                0,
                84,
                w,
                b.as_str(),
                FontId::UiBold,
                if held { g.c(Role::OnAccent) } else { dim },
                Align::Center,
            );
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for LiveScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.drawn = None;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                cx.svc.seq_mut().seq.set_fill(false);
                return Nav::Pop;
            }
            Event::Key(Key::Ok, KeyPhase::Down) => cx.svc.seq_mut().seq.set_fill(true),
            Event::Key(Key::Ok, KeyPhase::Up { .. }) => cx.svc.seq_mut().seq.set_fill(false),
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Down | KeyPhase::Repeat) => {
                let p = &mut cx.svc.seq_mut().project;
                let d = if k == Key::Next { 1 } else { PATTERNS - 1 };
                p.set_current((p.current() + d) % PATTERNS);
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                let p = &mut cx.svc.seq_mut().project;
                p.set_tempo(tempo_step(p.tempo(), k == Key::Up, false));
            }
            Event::Ui(UiEvent::Menu { id: M_LIVE_PLAY, .. }) => toggle_play(cx.svc),
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let st = cx.svc.seq();
        let key = (st.project.rev(), playhead(st), st.seq.fill(), st.seq.counting_in());
        if dirty == Dirty::Changed && self.drawn == Some(key) {
            return;
        }
        self.draw_all(st, g);
        self.drawn = Some(key);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_none() {
            let l = self.lang;
            out.action(
                M_LIVE_PLAY,
                l.get(if cx.svc.seq().seq.playing() { Str::SeqStop } else { Str::SeqPlay }),
            );
        }
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: true,
            title: Some(self.lang.get(Str::SeqLive)),
            strip: StripUse::Hint(self.lang.get(Str::SeqHintLive)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume::default()
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, _r: &Resume) {
        self.lang = cx.svc.lang();
        self.drawn = None;
    }

    /// The pattern playing and the one that comes next, the tempo.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let st = cx.svc.seq();
        let p = &st.project;
        let l = self.lang;
        d.title(l.get(Str::SeqLive));
        let playing = st.seq.position().map_or(p.current() as u8, |x| x.pattern);
        d.say_u32(l.get(Str::SeqPattern), u32::from(playing) + 1);
        if p.mode() == PlayMode::Song {
            d.say(l.get(Str::SeqSong));
        } else {
            d.say_u32(l.get(Str::SeqNextPattern), p.current() as u32 + 1);
        }
        let mut v = StrBuf::<8>::new();
        v.push_u32(u32::from(p.tempo()));
        d.focus(orpheus_ui::a11y::Role::Slider, l.get(Str::SeqTempo)).value(v.as_str());
    }
}
