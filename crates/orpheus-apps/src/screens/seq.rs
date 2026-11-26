//! The Sequencer's screens (05 §7.16, SEQ-1; feature `seq`): the project
//! list, the grid, and four value lists — transport, step, track, mixer.
//!
//! The model is the service's (`SeqService::seq`, an
//! `orpheus_seq::Studio`): screens hold only their cursor, so any of them
//! is rebuilt from it after being covered. Edits go straight into the
//! project; the player reads it at each step (they are heard at the next
//! step). Playing goes on whatever the screen shows.
//!
//! Grid keys (05 §7.16): Prev/Next move along the steps (held: by pages),
//! Up/Down change track, OK toggles the step, hold OK edits it, Menu the
//! pattern and track menu (play/stop first), Back saves and returns to
//! the list.

use orpheus_seq::model::{
    MAX_RATCHET, MAX_STEPS, MICRO_MAX, PAGE, PATTERNS, PROB_LEVELS, SWING_MAX, SWING_MIN, TRACKS,
};
use orpheus_seq::model::{PlayMode, Project, note_label};
use orpheus_seq::orpheus_synth::kit::FACTORY;
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
use super::{SEQ_GRID, SEQ_MIXER, SEQ_STEP, SEQ_TRACK, SEQ_TRANSPORT};
use crate::seq::{SeqRequest, SeqResult, new_name, push_tempo, push_track, tempo_step};
use crate::services::{Services, other};
use crate::strings::{Lang, Str};

/// Rows visible in the content area.
const VISIBLE: u32 = 5;

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

/// Save the open project if it has changes.
fn autosave<S: Services + ?Sized>(svc: &mut S) {
    if svc.seq().dirty() {
        svc.seq_request(SeqRequest::Save);
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
            cx.svc.seq_mut().new_project(new_name(no).as_str());
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
        if out.parent().is_none() && self.list.focus > 0 && !cx.svc.seq().list.items().is_empty() {
            out.action(M_DELETE, self.lang.get(Str::SeqDelete));
        }
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
}

// ------------------------------------------------------------ the grid

/// Steps across the grid (one page).
const COLS: usize = PAGE;
/// Header height.
const HEAD: i16 = 14;
/// A track row's height.
const ROW: i16 = 12;
/// A step cell's pitch.
const CELL: i16 = 7;
/// Left edge of the cells (the track labels before it).
const GRID_X: i16 = 16;
/// Top of the footer.
const FOOT: i16 = HEAD + ROW * TRACKS as i16;

const M_PLAY: u16 = 1;
const M_TRANSPORT: u16 = 2;
const M_MIXER: u16 = 3;
const M_TRACK: u16 = 4;
const M_NEXT: u16 = 5;
const M_PREV: u16 = 6;
const M_COPY: u16 = 7;
const M_CLEAR: u16 = 8;
const M_SAVE: u16 = 9;

/// What the grid shows: redrawn when it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GridKey {
    rev: u32,
    track: u8,
    step: u8,
    play: Option<(u8, u8)>,
}

/// The grid: 16 steps × 8 tracks, the header and the page footer.
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

impl GridScreen {
    fn len(p: &Project) -> usize {
        p.pattern(p.current()).len()
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
        let name = note_label(p.track(usize::from(self.track)).note).1;
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
        for t in 0..TRACKS {
            let y = HEAD + ROW * t as i16;
            g.fill_role(Rect::new(0, y, 128, ROW), Role::Bg);
            let tr = p.track(t);
            let c = if t == usize::from(self.track) {
                accent
            } else if tr.solo {
                good
            } else if !p.audible(t) {
                g.pal.mix(Role::Bg, Role::TextDim, 110)
            } else {
                dim
            };
            // Lower case: two capitals of the UI font are wider than the
            // 16 px column.
            let mut lab = StrBuf::<4>::new();
            for ch in note_label(tr.note).0.chars() {
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
                let st_ = pat.step(t, s);
                let here = play_step == Some(s);
                if !st_.on() {
                    let a = if here {
                        170
                    } else if s % 4 == 0 {
                        80
                    } else {
                        45
                    };
                    g.fill(cell, g.pal.mix(Role::Bg, Role::TextDim, a));
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
                // Ratchets: the cell split by gaps.
                for k in 1..i16::from(st_.ratchet()) {
                    let gy = y + 1 + k * (ROW - 2) / i16::from(st_.ratchet());
                    g.hline(x, gy, CELL - 1, g.c(Role::Bg));
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
        // Footer: pages, the step and its velocity.
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
        let s = pat.step(usize::from(self.track), usize::from(self.step));
        if s.on() {
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
                let n = TRACKS as u8;
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
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
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
        self.track = (r.focus as usize).min(TRACKS - 1) as u8;
        self.step = (r.top as usize).min(MAX_STEPS - 1) as u8;
        self.clamp(&cx.svc.seq().project);
    }
}

impl GridScreen {
    fn menu_chosen<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, id: u16) -> Nav {
        let cur = cx.svc.seq().project.current();
        match id {
            M_PLAY => cx.svc.seq_mut().toggle_play(),
            M_TRANSPORT => return Nav::Push(SEQ_TRANSPORT, Arg(0)),
            M_MIXER => return Nav::Push(SEQ_MIXER, Arg(0)),
            M_TRACK => return Nav::Push(SEQ_TRACK, Arg(u32::from(self.track))),
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
    /// Play/stop, tempo, swing, tap tempo, play mode, pattern, length, kit.
    Transport,
    /// The step of the grid's cursor (arg: track << 8 | step).
    Step,
    /// A track's sound, level, mute, solo (arg: track).
    Track,
    /// Every track: mute (OK), solo (hold OK), level (Prev/Next).
    Mixer,
}

const T_PLAY: u16 = 1;
const T_TEMPO: u16 = 2;
const T_SWING: u16 = 3;
const T_TAP: u16 = 4;
const T_MODE: u16 = 5;
const T_PATTERN: u16 = 6;
const T_LENGTH: u16 = 7;
const T_KIT: u16 = 8;
const T_INFO: u16 = 20;
const T_ON: u16 = 21;
const T_VEL: u16 = 22;
const T_CHANCE: u16 = 23;
const T_RATCHET: u16 = 24;
const T_DELAY: u16 = 25;
const T_ACCENT: u16 = 26;
const T_SOUND: u16 = 30;
const T_LEVEL: u16 = 31;
const T_MUTE: u16 = 32;
const T_SOLO: u16 = 33;
const T_MIX: u16 = 40;

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
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            SeqEdit::Transport => "SeqTransport",
            SeqEdit::Step => "SeqStep",
            SeqEdit::Track => "SeqTrack",
            SeqEdit::Mixer => "SeqMixer",
        }
    }

    fn track(&self) -> usize {
        let t = if self.kind == SeqEdit::Step { self.arg >> 8 } else { self.arg };
        (t as usize).min(TRACKS - 1)
    }

    fn step(&self) -> usize {
        ((self.arg & 0xFF) as usize).min(MAX_STEPS - 1)
    }

    fn refresh(&mut self, st: &Studio) {
        let l = self.lang;
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
            }
            SeqEdit::Step => {
                let (t, s) = (self.track(), self.step());
                let step = p.pattern(p.current()).step(t, s);
                if let Some(r) = rows.push("", T_INFO) {
                    push_track(&mut r.value, p.track(t).note);
                    r.value.push_str(" · ");
                    r.value.push_str(l.get(Str::SeqStep));
                    r.value.push(' ');
                    r.value.push_u32(s as u32 + 1);
                    r.dim = true;
                }
                if let Some(r) = rows.push(l.get(Str::SeqStepOn), T_ON) {
                    r.acc = Accessory::Toggle(step.on());
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
            }
            SeqEdit::Track => {
                let t = self.track();
                let tr = p.track(t);
                if let Some(r) = rows.push(l.get(Str::SeqSound), T_SOUND) {
                    r.value.push_str(note_label(tr.note).1);
                }
                if let Some(r) = rows.push(l.get(Str::Level), T_LEVEL) {
                    r.value.push_u32(u32::from(tr.level));
                    r.value.push_str(" %");
                }
                if let Some(r) = rows.push(l.get(Str::SeqMute), T_MUTE) {
                    r.acc = Accessory::Toggle(tr.mute);
                }
                if let Some(r) = rows.push(l.get(Str::SeqSolo), T_SOLO) {
                    r.acc = Accessory::Toggle(tr.solo);
                }
            }
            SeqEdit::Mixer => {
                for t in 0..TRACKS {
                    let tr = p.track(t);
                    let Some(r) = rows.push(note_label(tr.note).1, T_MIX + t as u16) else { break };
                    if tr.solo {
                        r.value.push_str(l.get(Str::SeqSolo));
                        r.value.push_str(" · ");
                    } else if tr.mute {
                        r.value.push_str(l.get(Str::SeqMute));
                        r.value.push_str(" · ");
                    }
                    r.value.push_u32(u32::from(tr.level));
                    r.value.push_str(" %");
                    r.dim = !p.audible(t);
                }
            }
        }
        self.rows = rows;
        self.seen = (p.rev(), st.seq.playing());
    }

    fn focused(&self) -> u16 {
        self.rows.get(self.list.focus).map_or(0, |r| r.tag)
    }

    /// Prev/Next on the focused row.
    fn adjust(&mut self, st: &mut Studio, up: bool, fast: bool) {
        let p = &mut st.project;
        let d: i32 = if up { 1 } else { -1 };
        let cur = p.current();
        let (t, s) = (self.track(), self.step());
        let step = p.pattern(cur).step(t, s);
        let by = |v: u8, n: i32, lo: i32, hi: i32| (i32::from(v) + n).clamp(lo, hi) as u8;
        match self.focused() {
            T_TEMPO => p.set_tempo(tempo_step(p.tempo(), up, fast)),
            T_SWING => p.set_swing(by(p.swing(), d, i32::from(SWING_MIN), i32::from(SWING_MAX))),
            T_MODE => p.set_mode(if p.mode() == PlayMode::Song { PlayMode::Pattern } else { PlayMode::Song }),
            T_PATTERN => p.set_current((cur as i32 + d).rem_euclid(PATTERNS as i32) as usize),
            T_LENGTH => {
                let n = p.pattern(cur).len() as i32 + if fast { 4 * d } else { d };
                p.set_len(cur, n.clamp(1, MAX_STEPS as i32) as usize);
            }
            T_KIT => {
                let k =
                    p.factory_kit().map_or(0, |k| (k as i32 + d).rem_euclid(FACTORY.len() as i32) as usize);
                p.set_factory_kit(k);
            }
            T_VEL => p.set_step(
                cur,
                t,
                s,
                step.with_velocity(by(step.velocity(), if fast { 8 * d } else { d }, 1, 127)),
            ),
            T_CHANCE => p.set_step(cur, t, s, step.with_prob(by(step.prob(), d, 1, i32::from(PROB_LEVELS)))),
            T_RATCHET => {
                p.set_step(cur, t, s, step.with_ratchet(by(step.ratchet(), d, 1, i32::from(MAX_RATCHET))));
            }
            T_DELAY => p.set_step(cur, t, s, step.with_micro(by(step.micro(), d, 0, i32::from(MICRO_MAX)))),
            T_SOUND => {
                let kit = *p.kit();
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
            T_LEVEL => p.set_level(t, by(p.track(t).level, 5 * d, 0, 100)),
            tag if (T_MIX..T_MIX + TRACKS as u16).contains(&tag) => {
                let m = usize::from(tag - T_MIX);
                p.set_level(m, by(p.track(m).level, 5 * d, 0, 100));
            }
            _ => {}
        }
    }

    /// OK on the focused row.
    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, tag: u16) {
        let st = cx.svc.seq_mut();
        let p = &mut st.project;
        let cur = p.current();
        let (t, s) = (self.track(), self.step());
        let step = p.pattern(cur).step(t, s);
        match tag {
            T_PLAY => st.toggle_play(),
            T_TAP => {
                if let Some(tempo) = self.tap.tap(cx.now.0) {
                    p.set_tempo(tempo);
                }
            }
            T_MODE | T_KIT => self.adjust(st, true, false),
            T_ON => p.set_step(cur, t, s, step.with_on(!step.on())),
            T_ACCENT => p.set_step(cur, t, s, step.with_accent(!step.accent())),
            T_MUTE => p.set_mute(t, !p.track(t).mute),
            T_SOLO => p.set_solo(t, !p.track(t).solo),
            tag if (T_MIX..T_MIX + TRACKS as u16).contains(&tag) => {
                let m = usize::from(tag - T_MIX);
                p.set_mute(m, !p.track(m).mute);
            }
            _ => {}
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for SeqEditScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        self.arg = arg.0;
        self.refresh(cx.svc.seq());
        let mut l = self.list;
        let first = u32::from(self.kind == SeqEdit::Step);
        l.set_focus(&self.rows, first, VISIBLE, cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(k @ (Key::Prev | Key::Next), ph) => {
                if matches!(ph, KeyPhase::Down | KeyPhase::Repeat) {
                    self.adjust(cx.svc.seq_mut(), k == Key::Next, ph == KeyPhase::Repeat);
                    self.refresh(cx.svc.seq());
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            Event::Key(Key::Ok, KeyPhase::Hold) if self.kind == SeqEdit::Mixer => {
                let tag = self.focused();
                if (T_MIX..T_MIX + TRACKS as u16).contains(&tag) {
                    let m = usize::from(tag - T_MIX);
                    let p = &mut cx.svc.seq_mut().project;
                    p.set_solo(m, !p.track(m).solo);
                    self.refresh(cx.svc.seq());
                    self.list.invalidate();
                }
                return Nav::Stay;
            }
            Event::Tick(_) => {
                let st = cx.svc.seq();
                if self.seen != (st.project.rev(), st.seq.playing()) {
                    self.refresh(st);
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
            self.activate(cx, tag);
            self.refresh(cx.svc.seq());
            self.list.invalidate();
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
        });
        let hint =
            self.lang.get(if self.kind == SeqEdit::Mixer { Str::SeqHintMixer } else { Str::SeqHintValues });
        Wants { ticks: true, title: Some(title), strip: StripUse::Hint(hint), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, handle: self.arg, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.arg = r.handle;
        self.refresh(cx.svc.seq());
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, VISIBLE, cx.now);
        self.list = l;
    }
}
