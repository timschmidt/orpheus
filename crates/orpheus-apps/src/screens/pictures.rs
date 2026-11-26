//! Pictures › viewer (05 §7.6): one picture of the current listing, full
//! screen, decoded by the service straight into the frame buffer
//! (`PictureService`). Prev/Next go to the neighbouring pictures of the
//! listing, OK switches between fit (letterboxed) and fill (cropped),
//! Up/Down rotate by a quarter turn; the menu has the slideshow, its
//! interval, rotation, fill and the picture's details.
//!
//! The browser is the Files screen in its Pictures mode (folders and
//! pictures only). Not here yet: the thumbnail grid, 100 % with panning,
//! rotation remembered per file, wallpaper, delete.

use orpheus_ui::color::Color;
use orpheus_ui::event::{Event, Instant, Key, KeyPhase, ServiceEvent, Source, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, Wants};
use orpheus_ui::text::StrBuf;

use crate::browse::{EntryKind, FileRef};
use crate::icons;
use crate::services::{PicError, PicInfo, PicReq, PicSource, PicState, Services, data};
use crate::strings::{Lang, Str};

/// Slideshow intervals offered (seconds); the default is the second.
const INTERVALS: [u8; 4] = [2, 5, 10, 30];

// Menu ids.
/// The viewer is black in every theme; its text is light grey and white.
const WHITE: Color = 0xFFFF;
const DIM: Color = 0xAD55;

const M_SLIDESHOW: u16 = 1;
const M_INTERVAL: u16 = 2;
const M_INTERVAL_0: u16 = 20;
const M_ROT_LEFT: u16 = 3;
const M_ROT_RIGHT: u16 = 4;
const M_FILL: u16 = 5;
const M_DETAILS: u16 = 6;

/// Where a search for a picture ended.
enum Found {
    At(u32, FileRef),
    /// Row `.0` is not loaded yet (asked for; `Event::Data(BROWSE)` follows).
    Wait(u32),
    None,
}

/// A search for the next picture to show.
#[derive(Clone, Copy, Debug)]
struct Seek {
    from: u32,
    forward: bool,
    /// At the end of the listing, go on from the other end (slideshow).
    wrap: bool,
}

#[derive(Default)]
pub struct PictureScreen {
    lang: Lang,
    row: u32,
    file: Option<FileRef>,
    fill: bool,
    turns: u8,
    /// A final outcome for the current picture, fill and rotation (not
    /// `Shown`: the frame buffer does not keep a picture).
    result: Option<PicState>,
    info: Option<PicInfo>,
    /// Waiting for `Event::Data(PICTURE)`.
    pending: bool,
    /// The picture arrived in the frame buffer: mark it at the next draw.
    arrived: bool,
    seek: Option<Seek>,
    slideshow: bool,
    /// Index into [`INTERVALS`] + 1 (0: the default).
    interval: u8,
    shown_at: Instant,
}

impl PictureScreen {
    /// The screen argument: row of the listing to start at (the first
    /// picture at or after it is shown), and whether the slideshow runs.
    #[must_use]
    pub const fn arg(row: u32, slideshow: bool) -> u32 {
        (row & 0x7FFF_FFFF) | if slideshow { 0x8000_0000 } else { 0 }
    }

    fn interval_s(&self) -> u8 {
        let i = if self.interval == 0 { 1 } else { usize::from(self.interval - 1) };
        INTERVALS[i.min(INTERVALS.len() - 1)]
    }

    fn req(&self, f: FileRef) -> PicReq {
        PicReq { src: PicSource::File(f), fill: self.fill, turns: self.turns }
    }

    /// The nearest picture from `s.from` in its direction (within the
    /// loaded window; a row outside it is asked for).
    fn find<S: Services + ?Sized>(svc: &mut S, s: Seek) -> Found {
        let count = svc.listing().count;
        let mut i = s.from;
        loop {
            if i >= count {
                return Found::None;
            }
            match svc.entry(i) {
                None => {
                    let first = if s.forward { i } else { i.saturating_sub(8) };
                    svc.want(first, 9);
                    return Found::Wait(i);
                }
                Some(e) if e.kind == EntryKind::Image => return Found::At(i, e.file),
                Some(_) => {}
            }
            i = if s.forward {
                i + 1
            } else {
                match i.checked_sub(1) {
                    Some(j) => j,
                    None => return Found::None,
                }
            };
        }
    }

    /// Carry on with the search in `self.seek`; true if the picture changed.
    fn resolve<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> bool {
        let Some(s) = self.seek else { return false };
        match Self::find(cx.svc, s) {
            Found::At(row, f) => {
                self.seek = None;
                self.show(cx, row, f);
                true
            }
            Found::Wait(row) => {
                self.seek = Some(Seek { from: row, ..s });
                false
            }
            Found::None => {
                let count = cx.svc.listing().count;
                if s.wrap && count > 0 {
                    // Once around: the next search does not wrap again.
                    let from = if s.forward { 0 } else { count - 1 };
                    self.seek = Some(Seek { from, wrap: false, ..s });
                    return self.resolve(cx);
                }
                self.seek = None;
                if self.file.is_none() {
                    cx.ui.redraw();
                }
                false
            }
        }
    }

    fn show<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, row: u32, f: FileRef) {
        self.row = row;
        self.file = Some(f);
        self.turns = 0;
        self.result = None;
        self.info = None;
        self.pending = false;
        self.arrived = false;
        self.shown_at = cx.now;
        cx.svc.focus_hint(f);
        cx.ui.redraw();
    }

    fn step<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, forward: bool) {
        let from = if forward {
            self.row.saturating_add(1)
        } else {
            match self.row.checked_sub(1) {
                Some(r) => r,
                None if self.slideshow => u32::MAX,
                None => return,
            }
        };
        self.seek = Some(Seek { from, forward, wrap: self.slideshow });
        self.resolve(cx);
    }

    /// Same picture, new fill or rotation.
    fn again<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        self.result = None;
        self.pending = false;
        self.arrived = false;
        cx.ui.redraw();
    }

    fn message(&self, g: &mut Gfx<'_>, cause: Str, action: Option<Str>) {
        let (w, h) = g.size();
        let dim = DIM;
        g.icon2((w - 24) / 2, 20, &icons::PICTURES, dim);
        let text = WHITE;
        let cause = self.lang.get(cause);
        let mut y = 62;
        let pitch = g.line_pitch(cause, FontId::UiBold);
        g.wrap(cause, FontId::UiBold, w - 12, |g, line| {
            if y <= h - 2 {
                g.text_in(6, y, w - 12, line, FontId::UiBold, text, Align::Center);
            }
            y += pitch;
        });
        if let Some(a) = action {
            y += 2;
            let a = self.lang.get(a);
            let pitch = g.line_pitch(a, FontId::Ui);
            g.wrap(a, FontId::Ui, w - 12, |g, line| {
                if y <= h - 2 {
                    g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
                }
                y += pitch;
            });
        }
    }

    fn draw_state(&self, g: &mut Gfx<'_>, st: PicState) {
        match st {
            PicState::Busy => self.message(g, Str::PicBusy, Some(Str::PicBusyAction)),
            PicState::Failed(e) => {
                let why = match e {
                    PicError::Unsupported => Str::PicUnsupported,
                    PicError::Corrupt => Str::PicCorrupt,
                    PicError::TooLarge => Str::PicTooLarge,
                    PicError::Io => Str::PicIo,
                };
                self.message(g, Str::PicFailed, Some(why));
            }
            PicState::Nothing => self.message(g, Str::PicFailed, None),
            PicState::Pending => {
                let (w, h) = g.size();
                let dim = DIM;
                g.text_in(0, h / 2 + 4, w, self.lang.get(Str::PicLoading), FontId::Ui, dim, Align::Center);
            }
            PicState::Shown(_) => {}
        }
    }

    fn details(&self) -> StrBuf<48> {
        let mut b = StrBuf::new();
        if let Some(i) = self.info {
            b.push_u32(i.width);
            b.push_str(" × ");
            b.push_u32(i.height);
            if let Some(f) = i.format {
                b.push_str(" · ");
                b.push_str(f.name());
            }
        } else {
            b.push('—');
        }
        b
    }
}

impl<S: Services + ?Sized> Screen<S> for PictureScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        self.slideshow = arg.0 & 0x8000_0000 != 0;
        self.row = arg.0 & 0x7FFF_FFFF;
        self.seek = Some(Seek { from: self.row, forward: true, wrap: self.slideshow });
        self.resolve(cx);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Next, KeyPhase::Up { held: false }) => self.step(cx, true),
            Event::Key(Key::Prev, KeyPhase::Up { held: false }) => self.step(cx, false),
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                self.fill = !self.fill;
                self.again(cx);
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down) => {
                self.turns = if k == Key::Down { (self.turns + 1) % 4 } else { (self.turns + 3) % 4 };
                self.again(cx);
            }
            Event::Data(d) if d.tag == data::BROWSE => {
                self.resolve(cx);
            }
            Event::Data(d) if d.tag == data::PICTURE && self.pending => {
                self.pending = false;
                self.shown_at = cx.now;
                match cx.svc.picture_state() {
                    // Dropped (a key, or something covered the screen): ask
                    // again with the next full draw.
                    PicState::Pending => cx.ui.redraw(),
                    PicState::Shown(i) => {
                        self.info = Some(i);
                        self.arrived = true;
                    }
                    st => {
                        self.result = Some(st);
                        cx.ui.redraw();
                    }
                }
            }
            Event::Tick(now) => {
                if self.result == Some(PicState::Busy) && cx.svc.player().source == Source::Idle {
                    // The music stopped: the decoder memory is free again.
                    self.again(cx);
                    return Nav::Stay;
                }
                let due = now.since(self.shown_at) >= u64::from(self.interval_s()) * 1000;
                if self.slideshow && self.file.is_some() && !self.pending && self.seek.is_none() && due {
                    self.step(cx, true);
                    // Nothing else to show: try again after another interval.
                    self.shown_at = now;
                }
            }
            Event::Ui(UiEvent::Menu { id, value }) => match id {
                M_SLIDESHOW => {
                    self.slideshow = value != 0;
                    self.shown_at = cx.now;
                }
                M_ROT_LEFT | M_ROT_RIGHT => {
                    self.turns = if id == M_ROT_RIGHT { (self.turns + 1) % 4 } else { (self.turns + 3) % 4 };
                    self.again(cx);
                }
                M_FILL => {
                    self.fill = value != 0;
                    self.again(cx);
                }
                M_DETAILS => {
                    let d = self.details();
                    cx.ui.toast(d.as_str());
                }
                i if (M_INTERVAL_0..M_INTERVAL_0 + INTERVALS.len() as u16).contains(&i) => {
                    self.interval = u8::try_from(i - M_INTERVAL_0 + 1).unwrap_or(0);
                }
                _ => {}
            },
            Event::Service(ServiceEvent::CardRemoved) => return Nav::Home,
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let r = g.bounds();
        if dirty == Dirty::Changed {
            if self.arrived {
                self.arrived = false;
                g.mark(r);
            }
            return;
        }
        self.arrived = false;
        g.fill(r, 0);
        let Some(f) = self.file else {
            if self.seek.is_some() {
                self.draw_state(g, PicState::Pending);
            } else {
                self.message(g, Str::NoPictures, None);
            }
            return;
        };
        if let Some(st) = self.result {
            self.draw_state(g, st);
            return;
        }
        match cx.svc.show_picture(g, r, self.req(f)) {
            PicState::Pending => {
                self.pending = true;
                self.draw_state(g, PicState::Pending);
            }
            PicState::Shown(i) => {
                self.pending = false;
                self.info = Some(i);
            }
            st => {
                self.pending = false;
                self.result = Some(st);
                self.draw_state(g, st);
            }
        }
    }

    fn menu(&self, _cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        match out.parent() {
            None => {
                out.toggle(M_SLIDESHOW, l.get(Str::Slideshow), self.slideshow);
                out.sub(M_INTERVAL, l.get(Str::SlideInterval));
                out.action(M_ROT_LEFT, l.get(Str::RotateLeft));
                out.action(M_ROT_RIGHT, l.get(Str::RotateRight));
                out.toggle(M_FILL, l.get(Str::FillScreen), self.fill);
                out.action(M_DETAILS, l.get(Str::ViewDetails));
            }
            Some(M_INTERVAL) => {
                out.title(l.get(Str::SlideInterval));
                let cur = self.interval_s();
                for (i, s) in INTERVALS.iter().enumerate() {
                    let label = match s {
                        2 => "2 s",
                        5 => "5 s",
                        10 => "10 s",
                        _ => "30 s",
                    };
                    out.radio(M_INTERVAL_0 + u16::try_from(i).unwrap_or(0), label, *s == cur);
                }
            }
            Some(_) => {}
        }
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.slideshow || self.result == Some(PicState::Busy),
            full_screen: true,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        let mut r = Resume { focus: self.row, ..Resume::default() };
        if let Some(f) = self.file {
            r.data[..8].copy_from_slice(&f.to_bytes());
            r.data[8] = 1;
        }
        r.data[9] = u8::from(self.fill) | (self.turns << 1) | (u8::from(self.slideshow) << 3);
        r.data[10] = self.interval;
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let f = (r.data[8] == 1).then(|| FileRef::from_bytes(&r.data[..8]));
        self.fill = r.data[9] & 1 != 0;
        self.turns = (r.data[9] >> 1) & 3;
        self.slideshow = r.data[9] & 8 != 0;
        self.interval = r.data[10];
        let row = f.and_then(|f| cx.svc.position(f)).unwrap_or(r.focus);
        self.file = None;
        self.seek = Some(Seek { from: row, forward: true, wrap: false });
        self.resolve(cx);
        // `show` reset the rotation chosen before.
        self.turns = (r.data[9] >> 1) & 3;
    }

    /// The picture's name, size and format, or why it does not show.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let l = self.lang;
        d.title(l.get(Str::AppPictures));
        match self.result {
            Some(PicState::Busy) => {
                d.say(l.get(Str::PicBusy));
                d.say(l.get(Str::PicBusyAction));
                return;
            }
            Some(PicState::Failed(_) | PicState::Nothing) => {
                d.say(l.get(Str::PicFailed));
            }
            _ => {}
        }
        let name = cx.svc.entry(self.row).filter(|_| self.file.is_some()).map_or("", |e| e.name);
        if name.is_empty() {
            d.say(l.get(Str::PicLoading));
            return;
        }
        let count = cx.svc.listing().count;
        d.focus(orpheus_ui::a11y::Role::Text, name).value(self.details().as_str()).at(self.row, count);
        if self.slideshow {
            d.say(l.get(Str::Slideshow));
        }
    }
}
