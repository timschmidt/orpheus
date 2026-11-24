//! Music › Now Playing (05 §7.1) with its six views (Standard, Big art,
//! Lyrics, Up next, Details, Levels; Up/Down cycle, remembered), the
//! transport keys, seek with acceleration, the context menu, and the
//! equaliser screen.

use yoton_ui::event::{Event, Instant, Key, KeyPhase, Source, UiEvent};
use yoton_ui::font::FontId;
use yoton_ui::geom::Rect;
use yoton_ui::gfx::{Align, Gfx};
use yoton_ui::icons as ui_icons;
use yoton_ui::menu::MenuBuilder;
use yoton_ui::nav::{Arg, Cx, Dirty, Nav, Screen, StripUse, Wants};
use yoton_ui::text::StrBuf;
use yoton_ui::theme::Role;
use yoton_ui::widgets::dialog::DialogSpec;
use yoton_ui::widgets::list::{Item, ListAction, ListSource, ListView, Row};
use yoton_ui::widgets::misc::draw_progress;

use super::files::push_size;
use super::rows::Rows;
use super::{CHOICE, EQUALISER, FILES};
use crate::icons;
use crate::services::{ArtSize, FileOp, PlayerCmd, PlayerService, Services, data};
use crate::settings::{self, EQ_BANDS_HZ, NP_VIEWS, SettingId, id};
use crate::strings::{Lang, Str};

const VIEWS: u8 = 6;
const V_STANDARD: u8 = 0;
const V_BIG_ART: u8 = 1;
const V_LYRICS: u8 = 2;
const V_UP_NEXT: u8 = 3;
const V_DETAILS: u8 = 4;
const V_LEVELS: u8 = 5;
/// Big art shows its text overlay this long after a key (05 §7.1).
const OVERLAY_MS: u64 = 3000;
/// Levels redraw at most this often (≤ 15 fps).
const LEVELS_MS: u64 = 66;
const LIST_ROWS: u32 = 6;

fn push_time<const N: usize>(b: &mut StrBuf<N>, ms: u32) {
    b.push_duration(ms / 1000);
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Up next as a list source (rows from the current track on).
struct QueueSource<'a, S: ?Sized> {
    svc: &'a S,
    base: u32,
    len: u32,
}

impl<S: PlayerService + ?Sized> ListSource for QueueSource<'_, S> {
    fn len(&self) -> u32 {
        self.len
    }

    fn row(&self, i: u32) -> Row<'_> {
        match self.svc.queue_entry(self.base + i) {
            Some(q) => {
                let icon = if i == 0 { &ui_icons::PLAY } else { &ui_icons::NOTE };
                Row::Item(Item::new(q.title).icon(icon))
            }
            None => Row::Pending,
        }
    }
}

/// What the last draw showed, to redraw only on change.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Snap {
    view: u8,
    track: u32,
    source: Source,
    second: u32,
    bar: i16,
    overlay: bool,
    lyric: u32,
    qpos: u32,
    qlen: u32,
}

#[derive(Default)]
pub struct NowPlayingScreen {
    view: u8,
    lang: Lang,
    list: ListView,
    drawn: Option<Snap>,
    overlay_until: Instant,
    seek_repeats: u16,
    levels_at: Instant,
    now: Instant,
}

// Menu ids.
const M_SHUFFLE: u16 = 1;
const M_REPEAT: u16 = 2;
const M_REPEAT_0: u16 = 20;
const M_FAV: u16 = 3;
const M_BOOKMARK: u16 = 4;
const M_AB: u16 = 5;
const M_SET_A: u16 = 50;
const M_SET_B: u16 = 51;
const M_CLEAR_AB: u16 = 52;
const M_SPEED: u16 = 6;
const M_EQ: u16 = 7;
const M_SLEEP: u16 = 8;
const M_OUTPUT: u16 = 9;
const M_FOLDER: u16 = 10;
const M_DELETE: u16 = 11;
const M_DETAILS: u16 = 12;
const M_REMOVE: u16 = 13;
const M_UP: u16 = 14;
const M_DOWN: u16 = 15;
const DLG_DELETE: u16 = 11;

impl NowPlayingScreen {
    fn set_view<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, v: u8) {
        self.view = v % VIEWS;
        cx.svc.set_setting(id::NP_VIEW, i16::from(self.view));
        self.list = ListView::new();
        self.drawn = None;
        if self.view == V_UP_NEXT {
            let p = cx.svc.player();
            cx.svc.want_queue(p.queue_pos, LIST_ROWS + 2);
        }
        cx.ui.invalidate_all();
    }

    fn lyric_index<S: PlayerService + ?Sized>(svc: &S, pos: u32) -> u32 {
        let n = svc.lyrics_count();
        let mut cur = 0;
        for i in 0..n {
            match svc.lyric(i).and_then(|l| l.time_ms) {
                Some(t) if t <= pos => cur = i,
                Some(_) => break,
                None => return 0,
            }
        }
        cur
    }

    fn snap<S: Services + ?Sized>(&self, svc: &S) -> Snap {
        let p = svc.player();
        let track = svc.track().map_or(0, |t| hash(t.title) ^ hash(t.path));
        let bar =
            i16::try_from(u64::from(p.position_ms) * 116 / u64::from(p.duration_ms.max(1))).unwrap_or(0);
        Snap {
            view: self.view,
            track,
            source: p.source,
            second: p.position_ms / 1000,
            bar,
            overlay: self.now < self.overlay_until,
            lyric: if self.view == V_LYRICS { Self::lyric_index(svc, p.position_ms) } else { 0 },
            qpos: p.queue_pos,
            qlen: p.queue_len,
        }
    }

    fn details(svc: &(impl Services + ?Sized), lang: Lang) -> Rows {
        let mut rows = Rows::default();
        let p = svc.player();
        let Some(t) = svc.track() else { return rows };
        let mut add = |label: Str, f: &dyn Fn(&mut StrBuf<24>)| {
            if let Some(r) = rows.push(lang.get(label), 0) {
                f(&mut r.value);
                r.wide_value = true;
            }
        };
        add(Str::Codec, &|v| {
            v.push_str(t.codec);
        });
        add(Str::Bitrate, &|v| {
            v.push_u32(u32::from(t.bitrate_kbps));
            v.push(' ');
            v.push_str(lang.get(Str::Kbps));
        });
        add(Str::SampleRate, &|v| {
            v.push_u32(t.sample_rate / 1000);
            if t.sample_rate % 1000 != 0 {
                v.push('.');
                v.push_u32((t.sample_rate % 1000) / 100);
            }
            v.push_str(" kHz/");
            v.push_u32(u32::from(t.bits));
        });
        add(Str::Channels, &|v| {
            v.push_u32(u32::from(t.channels));
        });
        add(Str::ReplayGain, &|v| match t.gain_cdb {
            Some(g) => {
                v.push(if g < 0 { '−' } else { '+' });
                v.push_u32(g.unsigned_abs() / 100);
                v.push('.');
                v.push_u32_padded(g.unsigned_abs() % 100, 2);
                v.push_str(" dB");
            }
            None => {
                v.push('—');
            }
        });
        add(Str::FileLabel, &|v| {
            v.push_str_or_ellipsis(t.path.rsplit('/').next().unwrap_or(t.path));
        });
        add(Str::Size, &|v| push_size(v, t.size));
        add(Str::Cpu, &|v| {
            v.push_u32(u32::from(p.cpu_pct));
            v.push_str(" %");
        });
        add(Str::Buffer, &|v| {
            v.push_u32(u32::from(p.buffer_pct));
            v.push_str(" %");
        });
        rows
    }

    fn seek_step(&self) -> i32 {
        match self.seek_repeats {
            0..6 => 2000,
            6..12 => 5000,
            12..20 => 10_000,
            _ => 30_000,
        }
    }

    fn list_view(&self) -> bool {
        self.view == V_UP_NEXT || self.view == V_DETAILS
    }

    fn draw_art(g: &mut Gfx<'_>, art: Option<&[u16]>, r: Rect, radius: i16) {
        match art {
            Some(px) => g.image(r.x, r.y, r.w, px),
            None => {
                let surf = g.c(Role::Surface);
                g.fill_round(r, radius, surf);
                let dim = g.c(Role::TextDim);
                g.icon2(r.x + (r.w - 24) / 2, r.y + (r.h - 24) / 2, &icons::MUSIC, dim);
            }
        }
    }

    fn draw_idle(&self, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let dim = g.c(Role::TextDim);
        g.icon2((w - 24) / 2, 10, &icons::MUSIC, dim);
        let text = g.c(Role::Text);
        let title = self.lang.get(Str::NothingPlaying);
        let mut y = 52;
        let tp = g.line_pitch(title, FontId::UiBold);
        g.wrap(title, FontId::UiBold, w - 12, |g, line| {
            g.text_in(6, y, w - 12, line, FontId::UiBold, text, Align::Center);
            y += tp;
        });
        y += 2;
        let action = self.lang.get(Str::NothingPlayingAction);
        let pitch = g.line_pitch(action, FontId::Ui);
        g.wrap(action, FontId::Ui, w - 12, |g, line| {
            g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
            y += pitch;
        });
    }

    /// The progress row and times (Standard view), redrawn every second.
    fn draw_progress_rows<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let p = svc.player();
        g.fill_role(Rect::new(0, 76, w, 38), Role::Bg);
        let prog =
            u16::try_from(u64::from(p.position_ms) * 1000 / u64::from(p.duration_ms.max(1))).unwrap_or(0);
        let bar = Rect::new(6, 80, w - 12, 4);
        draw_progress(g, bar, prog, None, Role::Bg);
        for at in [p.a_ms, p.b_ms].into_iter().flatten() {
            let x = bar.x
                + i16::try_from(
                    u64::from(at) * u64::from(bar.w.unsigned_abs()) / u64::from(p.duration_ms.max(1)),
                )
                .unwrap_or(0);
            let c = g.c(Role::Warning);
            g.fill(Rect::new(x, bar.y - 2, 1, bar.h + 4), c);
        }
        let text = g.c(Role::Text);
        let mut a = StrBuf::<12>::new();
        push_time(&mut a, p.position_ms);
        g.text(6, 100, a.as_str(), FontId::Ui, text);
        let mut b = StrBuf::<12>::new();
        b.push('-');
        push_time(&mut b, p.duration_ms.saturating_sub(p.position_ms));
        g.text_in(w - 56, 100, 50, b.as_str(), FontId::Ui, text, Align::Right);
        let ic = if p.source == Source::Playing { &ui_icons::PLAY } else { &ui_icons::PAUSE };
        g.icon((w - 10) / 2, 91, ic, text);
    }

    fn draw_standard<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let Some(t) = svc.track() else { return };
        let p = svc.player();
        Self::draw_art(g, svc.art(ArtSize::Small), Rect::new(4, 4, 52, 52), 6);
        let tx = 62;
        let tw = w - tx - 4;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // Bold unless a word would have to be broken; then regular.
        let font = if t.title.split(' ').all(|w| crate::strings_fit(g, w, FontId::UiBold, tw)) {
            FontId::UiBold
        } else {
            FontId::Ui
        };
        let pitch = g.line_pitch(t.title, font);
        let mut lines = 0i16;
        g.wrap(t.title, font, tw, |g, line| {
            if lines < 2 {
                g.text_in(tx, 16 + pitch * lines, tw, line, font, text, Align::Left);
            }
            lines += 1;
        });
        if lines > 2 {
            // Third line would be cut: end the second with an ellipsis.
            g.fill_role(Rect::new(tx + tw - 10, 16 + pitch - 12, 10, 14), Role::Bg);
            g.text(tx + tw - 9, 16 + pitch, "…", FontId::Ui, text);
        }
        let mut y = 16 + pitch * lines.min(2);
        for s in [t.artist, t.album] {
            if !s.is_empty() && y <= 58 {
                g.text_in(tx, y, tw, s, FontId::Ui, dim, Align::Left);
                y += g.line_pitch(s, FontId::Ui);
            }
        }
        let mut badge = StrBuf::<24>::new();
        badge.push_str(t.codec);
        badge.push(' ');
        badge.push_u32(t.sample_rate / 1000);
        if t.sample_rate % 1000 != 0 {
            badge.push('.');
            badge.push_u32((t.sample_rate % 1000) / 100);
        }
        badge.push('/');
        badge.push_u32(u32::from(t.bits));
        g.text(6, 72, badge.as_str(), FontId::Ui, dim);
        let mut qp = StrBuf::<16>::new();
        qp.push_u32(p.queue_pos + 1);
        qp.push_str(" / ");
        qp.push_u32(p.queue_len);
        g.text_in(w - 60, 72, 54, qp.as_str(), FontId::Ui, dim, Align::Right);
        self.draw_progress_rows(svc, g);
    }

    fn draw_big_art<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, overlay: bool) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        Self::draw_art(g, svc.art(ArtSize::Full), Rect::new(0, 0, w, h), 0);
        if !overlay {
            return;
        }
        let Some(t) = svc.track() else { return };
        let p = svc.player();
        let band = Rect::new(0, h - 40, w, 40);
        let bg = g.c(Role::Bg);
        g.dim(band, bg, 210);
        let prog =
            u16::try_from(u64::from(p.position_ms) * 1000 / u64::from(p.duration_ms.max(1))).unwrap_or(0);
        let acc = g.c(Role::Accent);
        let track = g.pal.mix(Role::Bg, Role::TextDim, 90);
        g.fill(Rect::new(0, band.y, w, 2), track);
        let pw = i16::try_from(i32::from(w) * i32::from(prog) / 1000).unwrap_or(0);
        g.fill(Rect::new(0, band.y, pw, 2), acc);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(6, band.y + 17, w - 12, t.title, FontId::UiBold, text, Align::Left);
        g.text_in(6, band.y + 33, w - 12, t.artist, FontId::Ui, dim, Align::Left);
    }

    fn draw_lyrics<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        let n = svc.lyrics_count();
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        if n == 0 {
            g.text_in(4, h / 2, w - 8, self.lang.get(Str::NoLyrics), FontId::Ui, dim, Align::Center);
            return;
        }
        let p = svc.player();
        let cur = Self::lyric_index(svc, p.position_ms);
        let synced = svc.lyric(0).is_some_and(|l| l.time_ms.is_some());
        if !synced {
            let mut y = 14;
            for i in 0..n {
                let Some(l) = svc.lyric(i) else { break };
                if y > h - 2 {
                    break;
                }
                g.text_in(6, y, w - 12, l.text, FontId::Ui, text, Align::Center);
                y += g.line_pitch(l.text, FontId::Ui);
            }
            return;
        }
        // Current line centred and highlighted, neighbours dim.
        let centre = h / 2 + 4;
        if let Some(l) = svc.lyric(cur) {
            let pitch = g.line_pitch(l.text, FontId::UiBold);
            let nl = i16::try_from(g.wrap(l.text, FontId::UiBold, w - 12, |_, _| {}).min(2)).unwrap_or(1);
            let mut y = centre - (nl - 1) * pitch / 2;
            let mut k = 0;
            let acc = g.c(Role::Accent);
            let bar = Rect::new(2, y - 12 - 1, 3, pitch * nl);
            g.fill_round(bar, 1, acc);
            g.wrap(l.text, FontId::UiBold, w - 12, |g, line| {
                if k < 2 {
                    g.text_in(8, y, w - 14, line, FontId::UiBold, text, Align::Left);
                    y += pitch;
                }
                k += 1;
            });
            let mut above = centre - (nl - 1) * pitch / 2 - 20;
            let mut i = cur;
            while above > 8 && i > 0 {
                i -= 1;
                if let Some(l) = svc.lyric(i) {
                    g.text_in(8, above, w - 14, l.text, FontId::Ui, dim, Align::Left);
                }
                above -= 18;
            }
            let mut below = y + 6;
            let mut j = cur + 1;
            while below < h + 2 && j < n {
                if let Some(l) = svc.lyric(j) {
                    g.text_in(8, below, w - 14, l.text, FontId::Ui, dim, Align::Left);
                }
                below += 18;
                j += 1;
            }
        }
    }

    fn draw_levels<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let lv = svc.levels();
        let dim = g.c(Role::TextDim);
        let acc = g.c(Role::Accent);
        let warn = g.c(Role::Warning);
        let text = g.c(Role::Text);
        let track = g.pal.mix(Role::Bg, Role::TextDim, 60);
        for (ch, label) in ["L", "R"].iter().enumerate() {
            let y = 8 + i16::try_from(ch).unwrap_or(0) * 14;
            g.text(6, y + 9, label, FontId::UiBold, dim);
            let bar = Rect::new(18, y + 1, w - 24, 8);
            g.fill_round(bar, 2, track);
            let px = |v: u8| i16::try_from(i32::from(bar.w) * i32::from(v) / 255).unwrap_or(0);
            let peak = lv.peak[ch];
            let c = if peak >= 250 { warn } else { acc };
            if peak > 0 {
                g.fill_round(Rect::new(bar.x, bar.y, px(peak).max(4), bar.h), 2, c);
            }
            let hx = bar.x + px(lv.hold[ch]).min(bar.w - 2);
            g.fill(Rect::new(hx, bar.y, 2, bar.h), text);
        }
        // 16-band spectrum.
        let top = 42;
        let base = h - 6;
        let bw = 6;
        let x0 = (w - 16 * (bw + 1)) / 2;
        for (i, v) in lv.bands.iter().enumerate() {
            let x = x0 + i16::try_from(i).unwrap_or(0) * (bw + 1);
            let hh = i16::try_from(i32::from(base - top) * i32::from(*v) / 255).unwrap_or(0).max(1);
            g.fill(Rect::new(x, top, bw, base - top), track);
            g.fill(Rect::new(x, base - hh, bw, hh), acc);
        }
    }

    fn draw_up_next<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>, dirty: Dirty, now: Instant) {
        let p = svc.player();
        let src = QueueSource { svc, base: p.queue_pos, len: p.queue_len.saturating_sub(p.queue_pos) };
        let mut l = self.list;
        l.draw(&src, g, dirty, now, Role::Bg);
        self.list = l;
    }

    fn draw_details<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>, dirty: Dirty, now: Instant) {
        let rows = Self::details(svc, self.lang);
        let mut l = self.list;
        l.draw(&rows, g, dirty, now, Role::Bg);
        self.list = l;
    }
}

impl<S: Services + ?Sized> Screen<S> for NowPlayingScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.view = u8::try_from(cx.svc.setting(id::NP_VIEW)).unwrap_or(0) % VIEWS;
        self.now = cx.now;
        self.overlay_until = cx.now.add_ms(OVERLAY_MS);
        if self.view == V_UP_NEXT {
            let p = cx.svc.player();
            cx.svc.want_queue(p.queue_pos, LIST_ROWS + 2);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        self.now = cx.now;
        if let Event::Key(..) = ev {
            self.overlay_until = cx.now.add_ms(OVERLAY_MS);
        }
        let p = cx.svc.player();
        match *ev {
            Event::Tick(_) => return Nav::Stay,
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                if self.view == V_UP_NEXT {
                    cx.svc.command(PlayerCmd::Jump(p.queue_pos + self.list.focus));
                    self.list = ListView::new();
                } else {
                    cx.svc.command(PlayerCmd::PlayPause);
                }
                return Nav::Stay;
            }
            Event::Key(Key::Ok, KeyPhase::Hold) => {
                cx.svc.command(PlayerCmd::Stop);
                return Nav::Stay;
            }
            Event::Key(Key::Prev, KeyPhase::Up { held: false }) => {
                cx.svc.command(if p.position_ms > 3000 { PlayerCmd::SeekTo(0) } else { PlayerCmd::Prev });
                return Nav::Stay;
            }
            Event::Key(Key::Next, KeyPhase::Up { held: false }) => {
                cx.svc.command(PlayerCmd::Next);
                return Nav::Stay;
            }
            Event::Key(k @ (Key::Prev | Key::Next), KeyPhase::Hold | KeyPhase::Repeat) => {
                if matches!(*ev, Event::Key(_, KeyPhase::Hold)) {
                    self.seek_repeats = 0;
                } else {
                    self.seek_repeats = self.seek_repeats.saturating_add(1);
                }
                let step = self.seek_step();
                cx.svc.command(PlayerCmd::SeekBy(if k == Key::Next { step } else { -step }));
                return Nav::Stay;
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down) if !self.list_view() => {
                let v = if k == Key::Down { self.view + 1 } else { self.view + VIEWS - 1 };
                self.set_view(cx, v);
                return Nav::Stay;
            }
            Event::Data(d) if d.tag == data::QUEUE || d.tag == data::LYRICS => {
                self.list.invalidate();
                self.drawn = None;
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, value }) => return self.menu_pick(cx, id, value),
            Event::Ui(UiEvent::Dialog { id: DLG_DELETE, button: 1 }) => {
                if let Some(t) = cx.svc.track() {
                    let f = t.file;
                    cx.svc.command(PlayerCmd::Next);
                    if cx.svc.file_op(FileOp::Delete(f)) {
                        cx.ui.toast(self.lang.get(Str::Deleted));
                    }
                }
                return Nav::Stay;
            }
            Event::Service(_) => {
                self.drawn = None;
                return Nav::Stay;
            }
            _ => {}
        }
        if !self.list_view() {
            return Nav::Stay;
        }
        // Up next / Details: Up/Down move in the list; past either end they
        // change the view.
        let len = if self.view == V_UP_NEXT {
            p.queue_len.saturating_sub(p.queue_pos)
        } else {
            yoton_ui::widgets::list::ListSource::len(&Self::details(&*cx.svc, self.lang))
        };
        match *ev {
            Event::Key(Key::Up, KeyPhase::Down) if self.list.focus == 0 => {
                self.set_view(cx, self.view + VIEWS - 1);
                return Nav::Stay;
            }
            Event::Key(Key::Down, KeyPhase::Down) if self.list.focus + 1 >= len => {
                self.set_view(cx, self.view + 1);
                return Nav::Stay;
            }
            _ => {}
        }
        let act = if self.view == V_UP_NEXT {
            let src = QueueSource { svc: &*cx.svc, base: p.queue_pos, len };
            let mut l = self.list;
            let a = l.handle(&src, ev, LIST_ROWS, cx.now);
            self.list = l;
            a
        } else {
            let rows = Self::details(&*cx.svc, self.lang);
            let mut l = self.list;
            let a = l.handle(&rows, ev, LIST_ROWS, cx.now);
            self.list = l;
            a
        };
        if act == ListAction::Moved && self.view == V_UP_NEXT {
            cx.svc.want_queue(p.queue_pos + self.list.top, LIST_ROWS + 1);
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let svc = &*cx.svc;
        let snap = self.snap(svc);
        let p = svc.player();
        let idle = p.queue_len == 0 || svc.track().is_none();
        if self.list_view() && !idle {
            // Lists track their own changes; redraw fully on track change.
            let d = if self.drawn.is_some_and(|s| s.track == snap.track && s.qpos == snap.qpos) {
                dirty
            } else {
                self.list.invalidate();
                Dirty::All
            };
            if self.view == V_UP_NEXT {
                self.draw_up_next(svc, g, d, cx.now);
            } else {
                self.draw_details(svc, g, d, cx.now);
            }
            self.drawn = Some(snap);
            return;
        }
        if self.view == V_LEVELS && !idle {
            if dirty == Dirty::Changed && cx.now.since(self.levels_at) < LEVELS_MS {
                return;
            }
            self.levels_at = cx.now;
            self.draw_levels(svc, g);
            self.drawn = Some(snap);
            return;
        }
        let same = self.drawn == Some(snap);
        if dirty == Dirty::Changed && same {
            return;
        }
        let only_time = dirty == Dirty::Changed
            && self.view == V_STANDARD
            && self.drawn.is_some_and(|d| {
                Snap { second: snap.second, bar: snap.bar, source: snap.source, ..d } == snap
            });
        if only_time && !idle {
            self.draw_progress_rows(svc, g);
            self.drawn = Some(snap);
            return;
        }
        g.clear(Role::Bg);
        if idle {
            self.draw_idle(g);
        } else {
            match self.view {
                V_BIG_ART => self.draw_big_art(svc, g, snap.overlay),
                V_LYRICS => self.draw_lyrics(svc, g),
                _ => self.draw_standard(svc, g),
            }
        }
        self.drawn = Some(snap);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        let svc = &*cx.svc;
        match out.parent() {
            None => {
                if self.view == V_UP_NEXT && self.list.focus > 0 {
                    out.action(M_REMOVE, l.get(Str::RemoveFromQueue));
                    out.action(M_UP, l.get(Str::MoveUp));
                    out.action(M_DOWN, l.get(Str::MoveDown));
                }
                out.toggle(M_SHUFFLE, l.get(Str::Shuffle), svc.setting(id::SHUFFLE) != 0);
                out.sub(M_REPEAT, l.get(Str::Repeat));
                out.action(M_FAV, l.get(Str::AddFavourites));
                out.action(M_BOOKMARK, l.get(Str::BookmarkHere));
                out.sub(M_AB, l.get(Str::AbRepeat));
                let mut v = StrBuf::<40>::new();
                if let Some(d) = settings::def(id::SPEED) {
                    settings::format_value(d, svc.setting(id::SPEED), l, &mut v);
                }
                out.value(M_SPEED, l.get(Str::Speed), v.as_str());
                out.action(M_EQ, l.get(Str::EqualiserDots));
                out.action(M_SLEEP, l.get(Str::SleepTimerDots));
                if self.view != V_UP_NEXT || self.list.focus == 0 {
                    out.action(M_FOLDER, l.get(Str::GoToFolder));
                    out.action(M_DELETE, l.get(Str::DeleteFile));
                }
            }
            Some(M_REPEAT) => {
                out.title(l.get(Str::Repeat));
                let cur = svc.setting(id::REPEAT);
                for (i, s) in [Str::Off, Str::RepeatOne, Str::RepeatAll].iter().enumerate() {
                    let v = i16::try_from(i).unwrap_or(0);
                    out.radio(M_REPEAT_0 + u16::try_from(i).unwrap_or(0), l.get(*s), cur == v);
                }
            }
            Some(M_AB) => {
                out.title(l.get(Str::AbRepeat));
                out.action(M_SET_A, l.get(Str::SetA));
                out.action(M_SET_B, l.get(Str::SetB));
                out.action(M_CLEAR_AB, l.get(Str::ClearAb));
            }
            Some(_) => {}
        }
        let _ = M_OUTPUT;
        let _ = M_DETAILS;
    }

    fn wants(&self) -> Wants {
        let title = NP_VIEWS.get(usize::from(self.view)).and_then(|c| match c {
            settings::Choice::S(s) => Some(self.lang.get(*s)),
            _ => None,
        });
        Wants {
            ticks: true,
            full_screen: self.view == V_BIG_ART,
            strip: StripUse::Content,
            title,
            is_player: true,
        }
    }
}

impl NowPlayingScreen {
    fn menu_pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, mid: u16, value: u16) -> Nav {
        let lang = self.lang;
        let p = cx.svc.player();
        let sel = p.queue_pos + self.list.focus;
        match mid {
            M_REMOVE => cx.svc.command(PlayerCmd::Remove(sel)),
            M_UP => {
                cx.svc.command(PlayerCmd::MoveUp(sel));
                self.list.focus = self.list.focus.saturating_sub(1).max(1);
            }
            M_DOWN => {
                cx.svc.command(PlayerCmd::MoveDown(sel));
                self.list.focus += 1;
            }
            M_SHUFFLE => cx.svc.set_setting(id::SHUFFLE, i16::try_from(value).unwrap_or(0)),
            20..=22 => cx.svc.set_setting(id::REPEAT, i16::try_from(mid - M_REPEAT_0).unwrap_or(0)),
            M_FAV => {
                cx.svc.command(PlayerCmd::Favourite);
                cx.ui.toast(lang.get(Str::AddedFavourites));
            }
            M_BOOKMARK => {
                cx.svc.command(PlayerCmd::Bookmark);
                cx.ui.toast(lang.get(Str::BookmarkAdded));
            }
            M_SET_A => cx.svc.command(PlayerCmd::SetA),
            M_SET_B => cx.svc.command(PlayerCmd::SetB),
            M_CLEAR_AB => cx.svc.command(PlayerCmd::ClearAb),
            M_SPEED => return Nav::Push(CHOICE, Arg(u32::from(id::SPEED.0))),
            M_EQ => return Nav::Push(EQUALISER, Arg(0)),
            M_SLEEP => return Nav::Push(CHOICE, Arg(u32::from(id::SLEEP_TIMER.0))),
            M_OUTPUT => return Nav::Push(CHOICE, Arg(u32::from(id::OUTPUT.0))),
            M_FOLDER => {
                if let Some(t) = cx.svc.track() {
                    return Nav::Push(FILES, Arg(t.file.dir));
                }
            }
            M_DELETE => {
                let name = cx.svc.track().map_or("", |t| t.path.rsplit('/').next().unwrap_or(t.path));
                cx.ui.dialog(DialogSpec::destructive(
                    DLG_DELETE,
                    lang.get(Str::DeleteQ),
                    name,
                    lang.get(Str::Cancel),
                    lang.get(Str::DeleteBtn),
                ));
            }
            M_DETAILS => self.set_view(cx, V_DETAILS),
            _ => {}
        }
        self.drawn = None;
        cx.ui.redraw();
        Nav::Stay
    }
}

/// Equaliser (05 §7.1): preset name, five bands (60 Hz … 14 kHz, ±12 dB),
/// pre-amp. Prev/Next choose the band, Up/Down change it (a built-in preset
/// is copied into Custom 1 first), OK opens the preset list.
#[derive(Default)]
pub struct EqualiserScreen {
    band: u8,
    lang: Lang,
    drawn: Option<(u8, [i8; 5], i16)>,
}

impl EqualiserScreen {
    fn gains<S: Services + ?Sized>(svc: &S) -> [i8; 5] {
        settings::eq_gains(svc.setting(id::EQUALISER), |k| svc.setting(k))
    }
}

fn band_key(preset: i16, band: u8) -> SettingId {
    let base = if preset == 10 { 5 } else { 0 };
    SettingId(id::EQ_BAND0.0 + base + u16::from(band))
}

impl<S: Services + ?Sized> Screen<S> for EqualiserScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                return Nav::Push(CHOICE, Arg(u32::from(id::EQUALISER.0)));
            }
            Event::Key(Key::Next, KeyPhase::Down | KeyPhase::Repeat) => self.band = (self.band + 1) % 5,
            Event::Key(Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => self.band = (self.band + 4) % 5,
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                let mut preset = cx.svc.setting(id::EQUALISER);
                let gains = Self::gains(&*cx.svc);
                if preset < 9 {
                    // Editing a built-in preset starts Custom 1 from it.
                    for (b, g) in gains.iter().enumerate() {
                        cx.svc.set_setting(band_key(9, u8::try_from(b).unwrap_or(0)), i16::from(*g));
                    }
                    preset = 9;
                    cx.svc.set_setting(id::EQUALISER, 9);
                }
                let key = band_key(preset, self.band);
                let v = cx.svc.setting(key) + if k == Key::Up { 1 } else { -1 };
                cx.svc.set_setting(key, v.clamp(-12, 12));
            }
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let gains = Self::gains(&*cx.svc);
        let preset = cx.svc.setting(id::EQUALISER);
        let state = (self.band, gains, preset);
        if dirty == Dirty::Changed && self.drawn == Some(state) {
            return;
        }
        let (w, _) = g.size();
        g.clear(Role::Bg);
        let l = self.lang;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let acc = g.c(Role::Accent);
        let mut name = StrBuf::<40>::new();
        if let Some(d) = settings::def(id::EQUALISER) {
            settings::format_value(d, preset, l, &mut name);
        }
        // Title row: preset (left), automatic pre-amp (right).
        let pre = -gains.iter().copied().max().unwrap_or(0).max(0);
        let mut p = StrBuf::<32>::new();
        p.push_str(l.get(Str::PreAmp));
        p.push(' ');
        p.push_i32(i32::from(pre));
        p.push(' ');
        p.push_str(l.get(Str::Db));
        g.text_in(6, 14, w - 12, name.as_str(), FontId::UiBold, text, Align::Center);
        g.text_in(6, 109, w - 12, p.as_str(), FontId::Ui, dim, Align::Center);
        // Bars: zero line at y 45, 1.5 px per dB.
        let zero = 45;
        let col = 24;
        let x0 = (w - 5 * col) / 2;
        for (b, gain) in gains.iter().enumerate() {
            let bi = u8::try_from(b).unwrap_or(0);
            let x = x0 + i16::from(bi) * col;
            let focused = bi == self.band;
            let under = if focused { Role::Surface } else { Role::Bg };
            if focused {
                let surf = g.c(Role::Surface);
                g.fill_round(Rect::new(x + 1, 20, col - 2, 76), 6, surf);
            }
            let track = g.pal.mix(under, Role::TextDim, 70);
            let cx_ = x + col / 2;
            g.fill_round(Rect::new(cx_ - 2, zero - 19, 4, 39), 2, track);
            let hpx = i16::from(*gain) * 3 / 2;
            let c = if focused { acc } else { dim };
            let r = if hpx >= 0 {
                Rect::new(cx_ - 2, zero - hpx, 4, hpx + 1)
            } else {
                Rect::new(cx_ - 2, zero, 4, 1 - hpx)
            };
            g.fill_round(r, 2, c);
            g.fill_disc(Rect::new(cx_ - 4, zero - hpx - 4, 8, 8), c);
            let mut v = StrBuf::<6>::new();
            if *gain > 0 {
                v.push('+');
            }
            v.push_i32(i32::from(*gain));
            g.text_in(x, 77, col, v.as_str(), FontId::Ui, if focused { text } else { dim }, Align::Center);
            let mut f = StrBuf::<6>::new();
            let hz = EQ_BANDS_HZ[b];
            if hz >= 1000 {
                f.push_u32(hz / 1000);
                if hz % 1000 != 0 {
                    f.push('.');
                    f.push_u32((hz % 1000) / 100);
                }
                f.push('k');
            } else {
                f.push_u32(hz);
            }
            g.text_in(x - 1, 91, col + 2, f.as_str(), FontId::Ui, dim, Align::Center);
        }
        self.drawn = Some(state);
    }

    fn wants(&self) -> Wants {
        Wants {
            strip: StripUse::Content,
            title: Some(self.lang.get(Str::Equaliser)),
            is_player: true,
            ..Wants::default()
        }
    }
}
