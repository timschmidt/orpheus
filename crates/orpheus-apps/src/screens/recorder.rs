//! The Recorder app (05 §7.5, WP UI-5b).
//!
//! ```text
//! │ ● Recording        MIC │   state · source
//! │       12:47            │   elapsed (large)
//! │ L ▮▮▮▮▮▮▮▮▮▯▯▯  -9 dB  │   meters with peak hold; clip in red
//! │ R ▮▮▮▮▮▮▮▮▯▯▯▯         │
//! │ WAV · 44.1 kHz · Mono  │
//! │ 5 h 12 min left        │
//! │ OK pause · Next mark   │   (strip)
//! ```
//!
//! The app captures for the meters while it is shown (Arm) and stops
//! when it is left (Disarm). OK starts; while recording OK pauses and
//! resumes; Next drops a mark; OK held or Back asks "Stop recording?"
//! (recordings are always saved). Up/Down: the manual input gain. Menu:
//! source, format, quality, automatic gain, voice activation, auto split,
//! monitor, low-cut, Recordings… (only while not recording).

use orpheus_ui::event::{Event, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;

use super::RECORDINGS;
use super::clocks::{draw_big_time, push_hms};
use crate::services::{
    RadioRequest, RecEnd, RecFormat, RecInfo, RecPhase, RecRequest, RecSource, Services, other, push_left,
    push_mhz, rec_event, rec_source,
};
use crate::settings::id;
use crate::strings::{Lang, Str};

const M_SOURCE: u16 = 1;
const M_FORMAT: u16 = 2;
const M_QUALITY: u16 = 3;
const M_AUTO_GAIN: u16 = 4;
const M_VOX: u16 = 5;
const M_SPLIT: u16 = 6;
const M_MONITOR: u16 = 7;
const M_LOWCUT: u16 = 8;
const M_LIST: u16 = 9;
const M_STOP: u16 = 10;
/// Sub-menu choices: `base + index`.
const M_SOURCE_0: u16 = 20;
const M_FORMAT_0: u16 = 30;
const M_QUALITY_0: u16 = 40;
const M_VOX_0: u16 = 50;
const M_SPLIT_0: u16 = 60;
const DLG_STOP: u16 = 0x500;

/// Meter scale floor (dBFS).
const FLOOR: i32 = -60;

const SOURCES: [(RecSource, Str); 3] =
    [(RecSource::Mic, Str::Microphone), (RecSource::Fm, Str::FmRadio), (RecSource::Line, Str::LineIn)];
const FORMATS: [&str; 3] = ["WAV", "WAV ADPCM", "FLAC"];
const QUALITIES: [Str; 3] = [Str::QVoice, Str::QStandard, Str::QHigh];
const VOX: [&str; 4] = ["", "−50 dB", "−40 dB", "−30 dB"];
const SPLIT: [&str; 4] = ["", "15 min", "30 min", "60 min"];

/// The source's short label.
fn source_label(info: &RecInfo, out: &mut StrBuf<16>) {
    match info.source {
        RecSource::Mic => {
            out.push_str("MIC");
        }
        RecSource::Line => {
            out.push_str("LINE");
        }
        RecSource::Fm => {
            out.push_str("FM");
            if info.fm_khz > 0 {
                out.push(' ');
                push_mhz(info.fm_khz, out);
            }
        }
    }
}

fn format_name(f: RecFormat) -> &'static str {
    match f {
        RecFormat::Wav => "WAV",
        RecFormat::Adpcm => "ADPCM",
        RecFormat::Flac => "FLAC",
    }
}

/// "44.1 kHz", "16 kHz".
fn push_rate<const N: usize>(rate: u32, out: &mut StrBuf<N>) {
    out.push_u32(rate / 1000);
    if rate % 1000 != 0 {
        out.push('.');
        out.push_u32(rate % 1000 / 100);
    }
    out.push_str(" kHz");
}

/// What the header and the time show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Head {
    phase: RecPhase,
    source: RecSource,
    fm_khz: u32,
    secs: u32,
    blink: bool,
    lang: &'static str,
}

/// What the meters show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Meters {
    bar: [i8; 2],
    hold: [i8; 2],
    clip: bool,
    stereo: bool,
}

/// The two text lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Lines {
    format: RecFormat,
    rate: u32,
    stereo: bool,
    phase: RecPhase,
    left_min: Option<u32>,
    drops: u32,
    lang: &'static str,
}

#[derive(Default)]
pub struct RecorderScreen {
    lang: Lang,
    head: Option<Head>,
    meters: Option<Meters>,
    lines: Option<Lines>,
}

impl RecorderScreen {
    fn arm<S: Services + ?Sized>(cx: &mut Cx<'_, S>) {
        // FM is recorded from the tuner: it has to play.
        if rec_source(cx.svc.setting(id::REC_SOURCE)) == RecSource::Fm && !cx.svc.radio().phase.is_on() {
            cx.svc.radio_request(RadioRequest::Start);
        }
        cx.svc.rec_request(RecRequest::Arm);
    }

    fn invalidate(&mut self) {
        self.head = None;
        self.meters = None;
        self.lines = None;
    }

    fn draw_head<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, h: &Head) {
        let (w, _) = g.size();
        g.fill_role(Rect::new(0, 0, w, 44), Role::Bg);
        let l = self.lang;
        let info = svc.recorder();
        let (label, role) = match h.phase {
            RecPhase::Recording => (Str::RecRecording, Role::Warning),
            RecPhase::Paused => (Str::RecPaused, Role::TextDim),
            RecPhase::Waiting => (Str::RecWaiting, Role::TextDim),
            RecPhase::Saving => (Str::RecSaving, Role::TextDim),
            RecPhase::Off | RecPhase::Armed => (Str::RecReady, Role::TextDim),
        };
        // The state dot: red while recording (blinking while paused),
        // hollow while ready.
        let dot = Rect::new(4, 4, 8, 8);
        match h.phase {
            RecPhase::Recording => {
                let c = g.c(Role::Warning);
                g.fill_disc(dot, c);
            }
            RecPhase::Paused | RecPhase::Waiting => {
                if h.blink {
                    let c = g.c(Role::Warning);
                    g.fill_disc(dot, c);
                } else {
                    let c = g.pal.mix(Role::Bg, Role::TextDim, 120);
                    g.fill_disc(dot, c);
                }
            }
            _ => {
                let c = g.c(Role::TextDim);
                g.fill_disc(dot, c);
                g.fill_disc(Rect::new(6, 6, 4, 4), g.c(Role::Bg));
            }
        }
        let mut src = StrBuf::<16>::new();
        source_label(&info, &mut src);
        let acc = g.c(Role::Accent);
        let sw = g.measure(src.as_str(), FontId::UiBold);
        g.text_in(w - sw - 4, 12, sw, src.as_str(), FontId::UiBold, acc, Align::Right);
        let c = g.c(role);
        g.text_in(16, 12, w - sw - 24, l.get(label), FontId::Ui, c, Align::Left);
        let mut t = StrBuf::<12>::new();
        push_hms(&mut t, h.secs);
        draw_big_time(g, 41, t.as_str());
    }

    fn draw_meters(g: &mut Gfx<'_>, m: &Meters, lang: Lang) {
        let (w, _) = g.size();
        g.fill_role(Rect::new(0, 44, w, 20), Role::Bg);
        let dim = g.c(Role::TextDim);
        let acc = g.c(Role::Accent);
        let warn = g.c(Role::Warning);
        let text = g.c(Role::Text);
        let track = g.pal.mix(Role::Bg, Role::TextDim, 60);
        // dB of the held peak, right of the bars.
        let mut d = StrBuf::<12>::new();
        let hold = i32::from(m.hold[0].max(m.hold[1]));
        if hold > FLOOR {
            d.push_i32(hold);
            d.push(' ');
            d.push_str(lang.get(Str::Db));
        } else {
            d.push_str("—");
        }
        let dw = 40;
        g.text_in(w - dw - 2, 56, dw, d.as_str(), FontId::Ui, if m.clip { warn } else { dim }, Align::Right);
        let px = |db: i8, width: i16| {
            let v = (i32::from(db).clamp(FLOOR, 0) - FLOOR) * i32::from(width) / -FLOOR;
            i16::try_from(v).unwrap_or(0)
        };
        let rows: &[(&str, usize, i16, i16)] =
            if m.stereo { &[("L", 0, 45, 7), ("R", 1, 55, 7)] } else { &[("", 0, 47, 10)] };
        for &(label, ch, y, hgt) in rows {
            let x0 = if label.is_empty() { 4 } else { 14 };
            if !label.is_empty() {
                g.text(3, y + 8, label, FontId::UiBold, dim);
            }
            let bar = Rect::new(x0, y, w - x0 - dw - 6, hgt);
            g.fill_round(bar, 2, track);
            let v = px(m.bar[ch], bar.w);
            if v > 0 {
                g.fill_round(Rect::new(bar.x, bar.y, v.max(4), bar.h), 2, if m.clip { warn } else { acc });
            }
            if i32::from(m.hold[ch]) > FLOOR {
                let hx = bar.x + px(m.hold[ch], bar.w).min(bar.w - 2);
                g.fill(Rect::new(hx, bar.y, 2, bar.h), text);
            }
        }
    }

    fn draw_lines(&self, g: &mut Gfx<'_>, li: &Lines) {
        let (w, _) = g.size();
        g.fill_role(Rect::new(0, 64, w, 32), Role::Bg);
        let l = self.lang;
        let dim = g.c(Role::TextDim);
        let mut a = StrBuf::<48>::new();
        a.push_str(format_name(li.format));
        a.push_str(" · ");
        push_rate(li.rate, &mut a);
        a.push_str(" · ");
        a.push_str(l.get(if li.stereo { Str::Stereo } else { Str::Mono }));
        g.text_in(4, 77, w - 8, a.as_str(), FontId::Ui, dim, Align::Left);
        let mut b = StrBuf::<48>::new();
        let mut role = Role::TextDim;
        if li.drops > 0 {
            b.push_u32(li.drops);
            b.push(' ');
            b.push_str(l.get(Str::RecDropped));
            role = Role::Warning;
        } else if let Some(m) = li.left_min {
            push_left(m * 60, l, &mut b);
            b.push(' ');
            b.push_str(l.get(Str::RecLeft));
        }
        let c = g.c(role);
        g.text_in(4, 92, w - 8, b.as_str(), FontId::Ui, c, Align::Left);
    }

    fn pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, mid: u16, value: u16) -> Nav {
        let l = self.lang;
        let set = |cx: &mut Cx<'_, S>, k, v: u16| cx.svc.set_setting(k, i16::try_from(v).unwrap_or(0));
        match mid {
            M_AUTO_GAIN => set(cx, id::REC_GAIN_MODE, u16::from(value != 0)),
            M_MONITOR => set(cx, id::REC_MONITOR, u16::from(value != 0)),
            M_LOWCUT => set(cx, id::REC_LOWCUT, u16::from(value != 0)),
            M_STOP => {
                cx.svc.rec_request(RecRequest::Stop);
                return Nav::Stay;
            }
            M_LIST => {
                if cx.svc.recorder().phase.is_recording() {
                    cx.ui.toast(l.get(Str::RecStopFirst));
                    return Nav::Stay;
                }
                return match cx.svc.rec_dir() {
                    Some(d) => {
                        cx.svc.rec_request(RecRequest::Disarm);
                        Nav::Push(RECORDINGS, Arg(d.0))
                    }
                    None => {
                        cx.ui.toast(l.get(Str::NoRecordings));
                        Nav::Stay
                    }
                };
            }
            m if (M_SOURCE_0..M_SOURCE_0 + 3).contains(&m) => set(cx, id::REC_SOURCE, m - M_SOURCE_0),
            m if (M_FORMAT_0..M_FORMAT_0 + 3).contains(&m) => set(cx, id::REC_FORMAT, m - M_FORMAT_0),
            m if (M_QUALITY_0..M_QUALITY_0 + 3).contains(&m) => set(cx, id::REC_QUALITY, m - M_QUALITY_0),
            m if (M_VOX_0..M_VOX_0 + 4).contains(&m) => set(cx, id::REC_VOX, m - M_VOX_0),
            m if (M_SPLIT_0..M_SPLIT_0 + 4).contains(&m) => set(cx, id::REC_SPLIT, m - M_SPLIT_0),
            _ => return Nav::Stay,
        }
        // A setting changed: capture again with it (while recording, the
        // service applies what can change live).
        Self::arm(cx);
        self.invalidate();
        Nav::Stay
    }

    fn ask_stop<S: Services + ?Sized>(&self, cx: &mut Cx<'_, S>) {
        let l = self.lang;
        cx.ui.dialog(DialogSpec::confirm(
            DLG_STOP,
            l.get(Str::StopRecQ),
            l.get(Str::StopRecText),
            l.get(Str::Cancel),
            l.get(Str::StopBtn),
        ));
    }
}

impl<S: Services + ?Sized> Screen<S> for RecorderScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.invalidate();
        Self::arm(cx);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let info = cx.svc.recorder();
        let l = self.lang;
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                if info.phase.is_recording() {
                    if info.phase != RecPhase::Saving {
                        self.ask_stop(cx);
                    }
                    return Nav::Stay;
                }
                cx.svc.rec_request(RecRequest::Disarm);
                Nav::Pop
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                match info.phase {
                    RecPhase::Off | RecPhase::Armed => cx.svc.rec_request(RecRequest::Start),
                    RecPhase::Recording | RecPhase::Waiting => cx.svc.rec_request(RecRequest::Pause),
                    RecPhase::Paused => cx.svc.rec_request(RecRequest::Resume),
                    RecPhase::Saving => {}
                }
                Nav::Stay
            }
            Event::Key(Key::Ok, KeyPhase::Hold) => {
                if info.phase.is_recording() && info.phase != RecPhase::Saving {
                    self.ask_stop(cx);
                }
                Nav::Stay
            }
            Event::Key(Key::Next, KeyPhase::Up { held: false }) => {
                if matches!(info.phase, RecPhase::Recording | RecPhase::Paused | RecPhase::Waiting) {
                    cx.svc.rec_request(RecRequest::Mark);
                    let mut t = StrBuf::<24>::new();
                    t.push_str(l.get(Str::RecMark));
                    t.push(' ');
                    t.push_u32(u32::from(info.marks) + 1);
                    cx.ui.toast(t.as_str());
                }
                Nav::Stay
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                if cx.svc.setting(id::REC_GAIN_MODE) != 0 {
                    cx.ui.toast(l.get(Str::AutoGain));
                    return Nav::Stay;
                }
                let d: i8 = if k == Key::Up { 1 } else { -1 };
                let v = (cx.svc.setting(id::REC_GAIN) + i16::from(d)).clamp(-12, 12);
                cx.svc.set_setting(id::REC_GAIN, v);
                cx.svc.rec_request(RecRequest::Gain(d));
                let mut t = StrBuf::<32>::new();
                t.push_str(l.get(Str::InputGain));
                t.push(' ');
                if v > 0 {
                    t.push('+');
                }
                t.push_i32(i32::from(v));
                t.push(' ');
                t.push_str(l.get(Str::Db));
                cx.ui.toast_for(t.as_str(), 1000);
                Nav::Stay
            }
            Event::Ui(UiEvent::Menu { id, value }) => self.pick(cx, id, value),
            Event::Ui(UiEvent::Dialog { id: DLG_STOP, button: 1 }) => {
                cx.svc.rec_request(RecRequest::Stop);
                Nav::Stay
            }
            Event::Service(ServiceEvent::Other { kind: other::RECORDER, value: rec_event::ENDED }) => {
                let s = match info.ended {
                    Some(RecEnd::CardFull) => Str::RecCardFull,
                    Some(RecEnd::CardError) => Str::RecCardError,
                    Some(RecEnd::NoCard) => Str::NoCard,
                    Some(RecEnd::NoSource) => Str::RecNoSource,
                    _ => Str::RecSaved,
                };
                let mut t = StrBuf::<64>::new();
                t.push_str(l.get(s));
                let name = cx.svc.rec_name();
                if s == Str::RecSaved && !name.is_empty() {
                    t.push_str(" · ");
                    t.push_str_or_ellipsis(name);
                }
                cx.ui.toast(t.as_str());
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let svc = &*cx.svc;
        let info = svc.recorder();
        if dirty == Dirty::All {
            g.clear(Role::Bg);
            self.invalidate();
        }
        let head = Head {
            phase: info.phase,
            source: info.source,
            fm_khz: info.fm_khz,
            secs: info.elapsed_ms / 1000,
            // Reduced motion (Animations off, A11Y-2): the dot stays lit.
            blink: !g.motion || (cx.now.0 / 500) % 2 == 0,
            lang: self.lang.code(),
        };
        if self.head != Some(head) {
            self.draw_head(svc, g, &head);
            self.head = Some(head);
        }
        let stereo = info.channels > 1;
        let m = Meters { bar: info.bar, hold: info.hold, clip: info.clip, stereo };
        if self.meters != Some(m) {
            Self::draw_meters(g, &m, self.lang);
            self.meters = Some(m);
        }
        let li = Lines {
            format: info.format,
            rate: info.rate,
            stereo,
            phase: info.phase,
            left_min: info.left_s.map(|s| s / 60),
            drops: info.drops,
            lang: self.lang.code(),
        };
        if self.lines != Some(li) {
            self.draw_lines(g, &li);
            self.lines = Some(li);
        }
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        let svc = &*cx.svc;
        let recording = svc.recorder().phase.is_recording();
        let cur = |k| usize::try_from(svc.setting(k)).unwrap_or(0);
        match out.parent() {
            None => {
                if recording {
                    out.action(M_STOP, l.get(Str::StopBtn));
                    out.toggle(M_AUTO_GAIN, l.get(Str::AutoGain), svc.setting(id::REC_GAIN_MODE) != 0);
                    out.sub(M_VOX, l.get(Str::VoiceActivation));
                    out.toggle(M_LOWCUT, l.get(Str::LowCut), svc.setting(id::REC_LOWCUT) != 0);
                    return;
                }
                out.sub(M_SOURCE, l.get(Str::RecSource));
                out.sub(M_FORMAT, l.get(Str::RecFormat));
                out.sub(M_QUALITY, l.get(Str::Quality));
                out.toggle(M_AUTO_GAIN, l.get(Str::AutoGain), svc.setting(id::REC_GAIN_MODE) != 0);
                out.sub(M_VOX, l.get(Str::VoiceActivation));
                out.sub(M_SPLIT, l.get(Str::AutoSplit));
                out.toggle(M_MONITOR, l.get(Str::Monitor), svc.setting(id::REC_MONITOR) != 0);
                out.toggle(M_LOWCUT, l.get(Str::LowCut), svc.setting(id::REC_LOWCUT) != 0);
                out.action(M_LIST, l.get(Str::RecordingsDots));
            }
            Some(M_SOURCE) => {
                out.title(l.get(Str::RecSource));
                let line = svc.rec_has_line_in();
                for (i, (_, s)) in SOURCES.iter().enumerate() {
                    if i == 2 && !line {
                        continue;
                    }
                    out.radio(
                        M_SOURCE_0 + u16::try_from(i).unwrap_or(0),
                        l.get(*s),
                        cur(id::REC_SOURCE) == i,
                    );
                }
            }
            Some(M_FORMAT) => {
                out.title(l.get(Str::RecFormat));
                for (i, f) in FORMATS.iter().enumerate() {
                    out.radio(M_FORMAT_0 + u16::try_from(i).unwrap_or(0), f, cur(id::REC_FORMAT) == i);
                }
            }
            Some(M_QUALITY) => {
                out.title(l.get(Str::Quality));
                for (i, q) in QUALITIES.iter().enumerate() {
                    out.radio(
                        M_QUALITY_0 + u16::try_from(i).unwrap_or(0),
                        l.get(*q),
                        cur(id::REC_QUALITY) == i,
                    );
                }
            }
            Some(M_VOX) => {
                out.title(l.get(Str::VoiceActivation));
                for (i, v) in VOX.iter().enumerate() {
                    let label = if i == 0 { l.get(Str::Off) } else { v };
                    out.radio(M_VOX_0 + u16::try_from(i).unwrap_or(0), label, cur(id::REC_VOX) == i);
                }
            }
            Some(M_SPLIT) => {
                out.title(l.get(Str::AutoSplit));
                for (i, m) in SPLIT.iter().enumerate() {
                    let label = if i == 0 { l.get(Str::Off) } else { m };
                    out.radio(M_SPLIT_0 + u16::try_from(i).unwrap_or(0), label, cur(id::REC_SPLIT) == i);
                }
            }
            Some(_) => {}
        }
    }

    fn wants(&self) -> Wants {
        let phase = self.head.map_or(RecPhase::Off, |h| h.phase);
        let hint = match phase {
            RecPhase::Recording | RecPhase::Waiting => Str::HintRecording,
            RecPhase::Paused => Str::HintRecPaused,
            _ => Str::HintRecIdle,
        };
        Wants {
            ticks: true,
            title: Some(self.lang.get(Str::AppRecorder)),
            strip: StripUse::Hint(self.lang.get(hint)),
            is_player: true,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume::default()
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, _r: &Resume) {
        self.lang = cx.svc.lang();
        self.invalidate();
        if !cx.svc.recorder().phase.is_recording() {
            Self::arm(cx);
        }
    }
}
