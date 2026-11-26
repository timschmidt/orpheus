//! The Radio app (05 §7.3, WP UI-5a): the frequency in large numerals,
//! band, signal bars and stereo, the preset and the station's RDS name and
//! radio text; the presets list.
//!
//! Keys: Prev/Next tap = one region step, hold = seek; Up/Down = previous /
//! next preset; OK = mute (start when off); OK hold = stop (the tuner is
//! powered down); Back leaves the app, the radio keeps playing (the status
//! bar shows FM). While a seek or an auto-scan runs, Prev/Next (and Back
//! during a scan) stop it. Menu: Save/Remove preset, Presets…, Auto-scan
//! (asks before replacing a list), Force mono, Region, Stop radio.
//!
//! The earphone cable is the antenna: without earphones the text line
//! says so (tuning still works).

use orpheus_ui::event::{Event, Instant, Key, KeyPhase, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row, marquee_offset};
use orpheus_ui::widgets::misc::{draw_big_value, draw_progress, draw_spinner};

use super::RADIO_PRESETS;
use crate::services::{
    PresetOp, RadioInfo, RadioPhase, RadioRequest, Region, Services, push_mhz, push_preset_name,
};
use crate::settings::id;
use crate::strings::{Lang, Str};

const M_SAVE: u16 = 1;
const M_REMOVE: u16 = 2;
const M_PRESETS: u16 = 3;
const M_SCAN: u16 = 4;
const M_MONO: u16 = 5;
const M_REGION: u16 = 6;
const M_STOP: u16 = 7;
const M_REGION_0: u16 = 10;
const DLG_SCAN: u16 = 0x300;

/// The text line (radio text, or the antenna note).
const TEXT_Y: i16 = 88;

/// The region names, in the setting's order.
const REGIONS: [Str; 5] = [Str::Europe, Str::UnitedStates, Str::World, Str::Japan, Str::Wide];

/// "P03".
fn push_preset_no<const N: usize>(i: usize, out: &mut StrBuf<N>) {
    out.push('P');
    out.push_u32_padded(u32::try_from(i + 1).unwrap_or(0), 2);
}

/// Everything shown except the marquee offset and the spinner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shown {
    phase: RadioPhase,
    khz: u32,
    bars: u8,
    stereo: bool,
    muted: bool,
    antenna: bool,
    progress: u16,
    found: u8,
    preset: Option<usize>,
    ps: u32,
    text: u32,
    lang: &'static str,
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

#[derive(Default)]
pub struct RadioScreen {
    lang: Lang,
    shown: Option<Shown>,
    /// Marquee of the text line: since when it shows this text, its
    /// overflow, the offset drawn.
    text_since: Instant,
    text_over: i16,
    scroll: i16,
    spin: Option<u8>,
    ticks: bool,
    scanning: bool,
}

impl RadioScreen {
    fn text<'a, S: Services + ?Sized>(&self, svc: &'a S, info: &RadioInfo) -> &'a str {
        if !info.antenna {
            self.lang.get(Str::RadioAntenna)
        } else if info.phase == RadioPhase::Playing {
            svc.radio_rt()
        } else {
            ""
        }
    }

    fn caption<S: Services + ?Sized>(&self, svc: &S, info: &RadioInfo, out: &mut StrBuf<48>) {
        let l = self.lang;
        match info.phase {
            RadioPhase::Off => {
                out.push_str(l.get(Str::RadioOff));
                out.push_str(" · ");
                out.push_str(l.get(Str::RadioOkToPlay));
            }
            RadioPhase::NotResponding => {
                out.push_str(l.get(Str::RadioNoTuner));
            }
            RadioPhase::Starting => {
                out.push_str(l.get(Str::BtStarting));
            }
            RadioPhase::Seeking => {
                out.push_str(l.get(Str::RadioSeeking));
            }
            RadioPhase::Scanning => {
                out.push_str(l.get(Str::RadioScanning));
                out.push(' ');
                out.push_u32(u32::from(info.found));
                out.push(' ');
                out.push_str(l.get(Str::RadioFound));
            }
            RadioPhase::Playing => {
                if info.muted {
                    out.push_str(l.get(Str::RadioMuted));
                } else {
                    out.push_str_or_ellipsis(svc.radio_ps());
                }
            }
        }
    }

    fn draw_all<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>, info: &RadioInfo, sh: &Shown) {
        let (w, _) = g.size();
        g.clear(Role::Bg);
        let acc = g.c(Role::Accent);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let on = info.phase.is_on();
        // Band, signal bars, stereo; the preset on the right.
        let fm = if on { acc } else { dim };
        let e = g.text(4, 13, "FM", FontId::UiBold, fm);
        let off_bar = g.pal.mix(Role::Bg, Role::TextDim, 90);
        for i in 0..3i16 {
            let h = 4 + 3 * i;
            let c = if on && i < i16::from(sh.bars) { text } else { off_bar };
            g.fill(Rect::new(e + 5 + i * 5, 13 - h, 3, h), c);
        }
        if on && sh.stereo {
            g.text(e + 23, 13, "ST", FontId::UiBold, text);
        }
        if let Some(i) = sh.preset {
            let mut p = StrBuf::<8>::new();
            push_preset_no(i, &mut p);
            g.text_in(w - 44, 13, 40, p.as_str(), FontId::UiBold, acc, Align::Right);
        }
        // The frequency, and under it the state or the station's name.
        let mut mhz = StrBuf::<8>::new();
        push_mhz(info.khz, &mut mhz);
        let mut cap = StrBuf::<48>::new();
        self.caption(svc, info, &mut cap);
        draw_big_value(g, Rect::new(0, 16, w, 50), mhz.as_str(), "MHz", cap.as_str(), Role::Bg);
        match info.phase {
            RadioPhase::Scanning => {
                draw_progress(g, Rect::new(12, 76, w - 24, 5), info.progress, None, Role::Bg);
            }
            RadioPhase::Starting | RadioPhase::Seeking => {}
            _ => self.draw_text(svc, g, info),
        }
    }

    fn draw_text<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, info: &RadioInfo) {
        let (w, _) = g.size();
        let r = Rect::new(4, TEXT_Y - 13, w - 8, 17);
        g.fill_role(r, Role::Bg);
        let s = self.text(svc, info);
        let c = g.c(if info.antenna { Role::Text } else { Role::Warning });
        if self.text_over > 0 {
            let x = 4 - self.scroll;
            g.with_clip(r, |g| {
                g.text(x, TEXT_Y, s, FontId::Ui, c);
            });
        } else {
            g.text_in(4, TEXT_Y, w - 8, s, FontId::Ui, c, Align::Center);
        }
    }

    fn pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, mid: u16, value: u16) -> Nav {
        let l = self.lang;
        let info = cx.svc.radio();
        match mid {
            M_SAVE => {
                if cx.svc.preset_op(PresetOp::Save(info.khz)) {
                    let mut t = StrBuf::<48>::new();
                    t.push_str(l.get(Str::PresetSaved));
                    if let Some(i) = cx.svc.presets().find(info.khz) {
                        t.push_str(" · ");
                        push_preset_no(i, &mut t);
                    }
                    cx.ui.toast(t.as_str());
                } else {
                    cx.ui.toast(l.get(Str::PresetsFull));
                }
            }
            M_REMOVE => {
                if let Some(i) = cx.svc.presets().find(info.khz)
                    && cx.svc.preset_op(PresetOp::Remove(i))
                {
                    cx.ui.toast(l.get(Str::Deleted));
                }
            }
            M_PRESETS => return Nav::Push(RADIO_PRESETS, Arg(0)),
            M_SCAN => {
                if cx.svc.presets().is_empty() {
                    cx.svc.radio_request(RadioRequest::Scan);
                } else {
                    cx.ui.dialog(DialogSpec::confirm(
                        DLG_SCAN,
                        l.get(Str::AutoScanQ),
                        l.get(Str::AutoScanText),
                        l.get(Str::Cancel),
                        l.get(Str::ScanBtn),
                    ));
                }
            }
            M_MONO => cx.svc.set_setting(id::FORCE_MONO, i16::from(value != 0)),
            M_STOP => cx.svc.radio_request(RadioRequest::Stop),
            m if (M_REGION_0..M_REGION_0 + 5).contains(&m) => {
                cx.svc.set_setting(id::RADIO_REGION, i16::try_from(m - M_REGION_0).unwrap_or(0));
            }
            _ => {}
        }
        Nav::Stay
    }
}

impl<S: Services + ?Sized> Screen<S> for RadioScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.shown = None;
        // Opening the app plays the radio (music stops).
        if !cx.svc.radio().phase.is_on() {
            cx.svc.radio_request(RadioRequest::Start);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let info = cx.svc.radio();
        let busy = matches!(info.phase, RadioPhase::Seeking | RadioPhase::Scanning);
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                if info.phase == RadioPhase::Scanning {
                    cx.svc.radio_request(RadioRequest::Cancel);
                    return Nav::Stay;
                }
                Nav::Pop
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                if info.phase.is_on() {
                    if !busy {
                        cx.svc.radio_request(RadioRequest::Mute(!info.muted));
                    }
                } else {
                    cx.svc.radio_request(RadioRequest::Start);
                }
                Nav::Stay
            }
            Event::Key(Key::Ok, KeyPhase::Hold) => {
                if info.phase.is_on() {
                    cx.svc.radio_request(RadioRequest::Stop);
                    cx.ui.toast(self.lang.get(Str::RadioOff));
                }
                Nav::Stay
            }
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Up { held: false }) => {
                if busy {
                    cx.svc.radio_request(RadioRequest::Cancel);
                } else {
                    cx.svc.radio_request(RadioRequest::Step { up: k == Key::Next });
                }
                Nav::Stay
            }
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Hold) => {
                if info.phase != RadioPhase::Scanning {
                    cx.svc.radio_request(RadioRequest::Seek { up: k == Key::Next });
                }
                Nav::Stay
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                if info.phase == RadioPhase::Scanning {
                    return Nav::Stay;
                }
                let p = cx.svc.presets();
                match p.neighbour(info.khz, k == Key::Down).and_then(|i| p.get(i)) {
                    Some(pr) => cx.svc.radio_request(RadioRequest::Tune(pr.khz)),
                    None => cx.ui.toast(self.lang.get(Str::NoPresets)),
                }
                Nav::Stay
            }
            Event::Ui(UiEvent::Menu { id, value }) => self.pick(cx, id, value),
            Event::Ui(UiEvent::Dialog { id: DLG_SCAN, button: 1 }) => {
                cx.svc.radio_request(RadioRequest::Scan);
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let svc = &*cx.svc;
        let info = svc.radio();
        let preset = svc.presets().find(info.khz);
        let text = self.text(svc, &info);
        let sh = Shown {
            phase: info.phase,
            khz: info.khz,
            bars: info.bars(),
            stereo: info.stereo,
            muted: info.muted,
            antenna: info.antenna,
            progress: info.progress,
            found: info.found,
            preset,
            ps: hash(svc.radio_ps()),
            text: hash(text),
            lang: self.lang.code(),
        };
        let shows_text =
            matches!(info.phase, RadioPhase::Playing | RadioPhase::Off | RadioPhase::NotResponding);
        // The text line's marquee restarts when the text changes.
        if self.shown.is_none_or(|s| s.text != sh.text) {
            self.text_since = cx.now;
            let (w, _) = g.size();
            self.text_over = (g.measure(text, FontId::Ui) - (w - 8)).max(0);
        }
        let scroll = if shows_text && g.motion {
            marquee_offset(cx.now.since(self.text_since), self.text_over)
        } else {
            0
        };
        let spinning = matches!(info.phase, RadioPhase::Starting | RadioPhase::Seeking);
        // Reduced motion (A11Y-2): the spinner is drawn once, still.
        let spin =
            spinning.then(|| if g.motion { u8::try_from((cx.now.0 / 125) % 8).unwrap_or(0) } else { 0 });
        self.scanning = info.phase == RadioPhase::Scanning;
        // Ticks while something moves (the firmware's UI loop also picks
        // up the radio's progress on them).
        self.ticks = spinning || self.scanning || (shows_text && self.text_over > 0);
        if dirty == Dirty::All || self.shown != Some(sh) {
            self.scroll = scroll;
            self.draw_all(svc, g, &info, &sh);
            self.shown = Some(sh);
            self.spin = None;
        } else if scroll != self.scroll && shows_text {
            self.scroll = scroll;
            self.draw_text(svc, g, &info);
        }
        if spin.is_some() && spin != self.spin {
            let (w, _) = g.size();
            draw_spinner(g, (w - 14) / 2, 72, cx.now, Role::Bg);
        }
        self.spin = spin;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        let svc = &*cx.svc;
        match out.parent() {
            None => {
                let info = svc.radio();
                if svc.presets().find(info.khz).is_some() {
                    out.action(M_REMOVE, l.get(Str::RemovePreset));
                } else {
                    out.action(M_SAVE, l.get(Str::SavePreset));
                }
                out.action(M_PRESETS, l.get(Str::PresetsDots));
                out.action(M_SCAN, l.get(Str::AutoScan));
                out.toggle(M_MONO, l.get(Str::ForceMono), svc.setting(id::FORCE_MONO) != 0);
                out.sub(M_REGION, l.get(Str::Region));
                if info.phase.is_on() {
                    out.action(M_STOP, l.get(Str::StopRadio));
                }
            }
            Some(M_REGION) => {
                out.title(l.get(Str::Region));
                let cur = Region::from_index(svc.setting(id::RADIO_REGION));
                for (i, (r, s)) in Region::ALL.iter().zip(REGIONS).enumerate() {
                    out.radio(M_REGION_0 + u16::try_from(i).unwrap_or(0), l.get(s), *r == cur);
                }
            }
            Some(_) => {}
        }
    }

    fn wants(&self) -> Wants {
        let hint = if self.scanning { Str::HintRadioScan } else { Str::HintRadio };
        Wants {
            ticks: self.ticks,
            title: Some(self.lang.get(Str::AppRadio)),
            strip: StripUse::Hint(self.lang.get(hint)),
            is_player: true,
            ..Wants::default()
        }
    }

    /// The frequency (Up/Down tune), the state or station, stereo, the
    /// radio text.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let svc = &*cx.svc;
        let info = svc.radio();
        d.title(self.lang.get(Str::AppRadio));
        let mut mhz = StrBuf::<16>::new();
        push_mhz(info.khz, &mut mhz);
        mhz.push_str(" MHz");
        let mut cap = StrBuf::<48>::new();
        self.caption(svc, &info, &mut cap);
        d.focus(orpheus_ui::a11y::Role::Slider, mhz.as_str()).value(cap.as_str());
        if info.phase.is_on() && info.stereo {
            d.say("stereo");
        }
        if !matches!(info.phase, RadioPhase::Starting | RadioPhase::Seeking | RadioPhase::Scanning) {
            d.say(self.text(svc, &info));
        }
    }
}

// ---------------------------------------------------------------- presets

/// Rows formatted at a time (the visible five and one more).
const WIN: usize = 6;
const VISIBLE: u32 = 5;
const M_UP: u16 = 1;
const M_DOWN: u16 = 2;
const M_DELETE: u16 = 3;

/// Radio › Presets…: frequency and the station's name in list order (the
/// row number is the preset number), the one playing marked; OK tunes and
/// goes back. Menu: Move up, Move down, Remove preset.
#[derive(Default)]
pub struct PresetsScreen {
    lang: Lang,
    list: ListView,
    n: u32,
    /// The preset playing.
    current: Option<u32>,
    /// First row of `win`.
    top: u32,
    win: [StrBuf<32>; WIN],
}

impl PresetsScreen {
    fn fill<S: Services + ?Sized>(&mut self, svc: &S) {
        let p = svc.presets();
        self.n = u32::try_from(p.len()).unwrap_or(0);
        self.current = p.find(svc.radio().khz).and_then(|i| u32::try_from(i).ok());
        self.top = self.list.top.min(self.list.focus);
        for (k, label) in self.win.iter_mut().enumerate() {
            label.clear();
            let i = usize::try_from(self.top).unwrap_or(0) + k;
            if let Some(pr) = p.get(i) {
                push_mhz(pr.khz, label);
                if !pr.name().is_empty() {
                    label.push_str(" · ");
                    push_preset_name(&pr, label);
                }
            }
        }
    }

    fn focused(&self) -> Option<usize> {
        (self.n > 0 && self.list.focus < self.n).then(|| usize::try_from(self.list.focus).unwrap_or(0))
    }
}

impl ListSource for PresetsScreen {
    fn len(&self) -> u32 {
        self.n.max(1)
    }

    fn row(&self, i: u32) -> Row<'_> {
        if self.n == 0 {
            return Row::Item(Item::new(self.lang.get(Str::NoPresets)).dim());
        }
        let k = i.checked_sub(self.top).and_then(|k| usize::try_from(k).ok());
        match k.and_then(|k| self.win.get(k)) {
            Some(label) if i < self.n => {
                let mut it = Item::new(label.as_str());
                if self.current == Some(i) {
                    it = it.accessory(Accessory::Radio(true));
                }
                Row::Item(it)
            }
            _ => Row::Pending,
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for PresetsScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.fill(&*cx.svc);
        // Focus the preset playing.
        let start = self.current.unwrap_or(0);
        let mut l = self.list;
        l.set_focus(self, start, VISIBLE, cx.now);
        self.list = l;
        self.fill(&*cx.svc);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Ui(UiEvent::Menu { id, .. }) => {
                let Some(i) = self.focused() else { return Nav::Stay };
                let (op, focus) = match id {
                    M_UP => (PresetOp::MoveUp(i), i.saturating_sub(1)),
                    M_DOWN => (PresetOp::MoveDown(i), i + 1),
                    M_DELETE => (PresetOp::Remove(i), i),
                    _ => return Nav::Stay,
                };
                if cx.svc.preset_op(op) {
                    self.fill(&*cx.svc);
                    let mut l = self.list;
                    l.set_focus(
                        self,
                        u32::try_from(focus).unwrap_or(0).min(self.n.saturating_sub(1)),
                        VISIBLE,
                        cx.now,
                    );
                    l.invalidate();
                    self.list = l;
                    if id == M_DELETE {
                        cx.ui.toast(self.lang.get(Str::Deleted));
                    }
                }
                self.fill(&*cx.svc);
                return Nav::Stay;
            }
            Event::Service(_) => {
                self.fill(&*cx.svc);
                self.list.invalidate();
                return Nav::Stay;
            }
            _ => {}
        }
        self.fill(&*cx.svc);
        let mut l = self.list;
        let act = l.handle(self, ev, VISIBLE, cx.list());
        self.list = l;
        self.fill(&*cx.svc);
        match act {
            ListAction::Activated(i) => {
                let p = cx.svc.presets().get(usize::try_from(i).unwrap_or(usize::MAX));
                match p {
                    Some(p) => {
                        cx.svc.radio_request(crate::services::RadioRequest::Tune(p.khz));
                        Nav::Pop
                    }
                    None => Nav::Stay,
                }
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        self.fill(&*cx.svc);
        let mut l = self.list;
        l.draw(self, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn menu(&self, _cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let Some(i) = self.focused() else { return };
        let l = self.lang;
        if i > 0 {
            out.action(M_UP, l.get(Str::MoveUp));
        }
        if i + 1 < usize::try_from(self.n).unwrap_or(0) {
            out.action(M_DOWN, l.get(Str::MoveDown));
        }
        out.action(M_DELETE, l.get(Str::RemovePreset));
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.get(Str::Presets)),
            strip: StripUse::Auto,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.fill(&*cx.svc);
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(self, r.focus, VISIBLE, cx.now);
        self.list = l;
        self.fill(&*cx.svc);
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, _cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(self.lang.get(Str::Presets));
        d.list(self, &self.list, "empty");
    }
}
