//! Music › Now Playing (05 §7.1) with its six views (Standard, Big art,
//! Lyrics, Up next, Details, Levels; Up/Down cycle, remembered), the
//! transport keys, seek with acceleration, the context menu, and the
//! equaliser screen.

use orpheus_ui::event::{Event, Instant, Key, KeyPhase, ServiceEvent, Source, UiEvent};
use orpheus_ui::font::{FontId, TextSize};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::icons as ui_icons;
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Item, ListAction, ListSource, ListView, Row};
use orpheus_ui::widgets::misc::draw_progress;

use super::files::push_size;
use super::rows::Rows;
use super::{CHOICE, EQUALISER, FILES, LIBRARY, LibraryScreen};
use crate::icons;
use crate::services::{ArtSize, FileOp, LibList, PicReq, PicState, PlayerCmd, PlayerService, Services, data};
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

/// The seek ladder of Prev/Next held (05 §7.1: "accelerating from 2 s/step
/// to 30 s/step"), in seconds; Settings › Playback › Seek step is its
/// first rung.
const SEEK_LADDER_S: [i32; 4] = [2, 5, 10, 30];

/// One seek step of Prev/Next held: Settings › Playback › Seek step
/// (0x0205: 2, 5, 10 or 30 s, 5 s by default) for the hold and five repeats,
/// then one rung up the ladder after 6, 12 and 20 repeats, at most 30 s.
/// With "2 s" it is the ladder of 05 §7.1 from its start.
#[must_use]
pub fn seek_step_ms(setting: i16, repeats: u16) -> i32 {
    let first = usize::try_from(setting).unwrap_or(0).min(SEEK_LADDER_S.len() - 1);
    let up = match repeats {
        0..6 => 0,
        6..12 => 1,
        12..20 => 2,
        _ => 3,
    };
    SEEK_LADDER_S[(first + up).min(SEEK_LADDER_S.len() - 1)] * 1000
}

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
    art: u32,
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
    /// Album art asked for and not there yet; arrived (mark it).
    art_pending: bool,
    art_arrived: bool,
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
const M_FOLDER: u16 = 10;
const M_DELETE: u16 = 11;
const M_DETAILS: u16 = 12;
const M_REMOVE: u16 = 13;
const M_UP: u16 = 14;
const M_DOWN: u16 = 15;
const M_ALBUM: u16 = 16;
const M_ARTIST: u16 = 17;
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
            art: p.art,
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

    fn list_view(&self) -> bool {
        self.view == V_UP_NEXT || self.view == V_DETAILS
    }

    /// Album art in `r`, or the placeholder (also while it is on its way:
    /// it replaces the placeholder in the frame buffer, see `art_arrived`).
    fn draw_art<S: Services + ?Sized>(
        &mut self,
        svc: &S,
        g: &mut Gfx<'_>,
        size: ArtSize,
        r: Rect,
        radius: i16,
    ) {
        let st =
            if svc.player().art == 0 { PicState::Nothing } else { svc.show_picture(g, r, PicReq::art(size)) };
        self.art_pending = st == PicState::Pending;
        if !matches!(st, PicState::Shown(_)) {
            let surf = g.c(Role::Surface);
            g.fill_round(r, radius, surf);
            let dim = g.c(Role::TextDim);
            g.icon2(r.x + (r.w - 24) / 2, r.y + (r.h - 24) / 2, &icons::MUSIC, dim);
        }
    }

    fn art_rect(&self, g: &Gfx<'_>) -> Rect {
        let (w, h) = g.size();
        if self.view == V_BIG_ART { Rect::new(0, 0, w, h) } else { Rect::new(4, 4, 52, 52) }
    }

    fn draw_idle(&self, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let title = self.lang.get(Str::NothingPlaying);
        // Large text (A11Y-2): the 16 px face at most (the lines below the
        // icon have no room for more).
        let size = g.text.at_most_large();
        let (bold, regular) = (FontId::UiBold.sized(size), FontId::Ui.sized(size));
        let mut y = 52;
        if size == TextSize::Normal {
            g.icon2((w - 24) / 2, 10, &icons::MUSIC, dim);
        } else {
            // No icon: the words need the room.
            y = 20;
        }
        let tp = g.line_pitch(title, bold);
        g.wrap(title, bold, w - 12, |g, line| {
            g.text_in(6, y, w - 12, line, bold, text, Align::Center);
            y += tp;
        });
        y += 2;
        let action = self.lang.get(Str::NothingPlayingAction);
        // In the large face only if every line fits above the strip.
        let n = i16::try_from(g.wrap(action, regular, w - 12, |_, _| {})).unwrap_or(9);
        let regular = if y + g.line_pitch(action, regular) * (n - 1) <= 110 { regular } else { FontId::Ui };
        let pitch = g.line_pitch(action, regular);
        g.wrap(action, regular, w - 12, |g, line| {
            g.text_in(6, y, w - 12, line, regular, dim, Align::Center);
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
        // Large text (A11Y-2): the times in the 16 px face.
        let (f, base, iy) =
            if g.text == TextSize::Normal { (FontId::Ui, 100, 91) } else { (FontId::Large, 103, 93) };
        let mut a = StrBuf::<12>::new();
        push_time(&mut a, p.position_ms);
        g.text(6, base, a.as_str(), f, text);
        let mut b = StrBuf::<12>::new();
        b.push('-');
        // From the whole elapsed seconds, so both times change together at
        // the once-a-second redraw (VER-4 finding F-UI-3).
        push_time(&mut b, p.duration_ms.saturating_sub(p.position_ms / 1000 * 1000));
        g.text_in(w - 56, base, 50, b.as_str(), f, text, Align::Right);
        let ic = if p.source == Source::Playing { &ui_icons::PLAY } else { &ui_icons::PAUSE };
        g.icon((w - 10) / 2, iy, ic, text);
    }

    /// The Standard view with large text (A11Y-2, 05 §7.14): no art; the
    /// title over the whole width in the large bold face (two lines, the
    /// second cut with an ellipsis), the artist (and the album when there is
    /// room) in the 16 px face, the queue place (Large only); the times in
    /// the 16 px face. The codec badge and the art are in Details and Big art.
    fn draw_standard_large<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let Some(t) = svc.track() else { return };
        let p = svc.player();
        self.art_pending = false;
        let xl = g.text == TextSize::ExtraLarge;
        let tw = w - 12;
        // Bold unless a word would have to be broken; then regular.
        // The largest face in which no word has to be broken: bold, else
        // regular, else (extra large) the 16 px bold face.
        let mut bold = FontId::LargeBold;
        for f in [g.ui(true), g.ui(false)] {
            if t.title.split(' ').all(|wd| crate::strings_fit(g, wd, f, tw)) {
                bold = f;
                break;
            }
        }
        let xl = xl && matches!(bold, FontId::XLarge | FontId::XLargeBold);
        let (first, pitch) = if xl { (22, 26) } else { (17, 19) };
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let title = t.title;
        let base = title.as_ptr() as usize;
        let total = g.wrap(title, bold, tw, |_, _| {});
        let mut i = 0i16;
        g.wrap(title, bold, tw, |g, line| {
            let y = first + pitch * i;
            if i == 1 && total > 2 {
                // More follows: the rest on the second line, cut.
                let rest = title.get(line.as_ptr() as usize - base..).unwrap_or(line);
                g.text_in(6, y, tw, rest, bold, text, Align::Left);
            } else if i < 2 {
                g.text_in(6, y, tw, line, bold, text, Align::Left);
            }
            i += 1;
        });
        let mut y = first + pitch * (i.min(2) - 1) + if xl { 24 } else { 20 };
        for s in [t.artist, t.album] {
            if !s.is_empty() && y <= if xl { 72 } else { 58 } {
                g.text_in(6, y, tw, s, FontId::Large, dim, Align::Left);
                y += 19;
            }
        }
        // The queue place under the names (not with the extra large title:
        // no room).
        if !xl {
            let mut qp = StrBuf::<16>::new();
            qp.push_u32(p.queue_pos + 1);
            qp.push_str(" / ");
            qp.push_u32(p.queue_len);
            g.text_in(6, 74, tw, qp.as_str(), FontId::Large, dim, Align::Right);
        }
        self.draw_progress_rows(svc, g);
    }

    fn draw_standard<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>) {
        if g.text != TextSize::Normal {
            self.draw_standard_large(svc, g);
            return;
        }
        let (w, _) = g.size();
        let Some(t) = svc.track() else { return };
        let p = svc.player();
        self.draw_art(svc, g, ArtSize::Small, Rect::new(4, 4, 52, 52), 6);
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

    fn draw_big_art<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>, overlay: bool) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        self.draw_art(svc, g, ArtSize::Full, Rect::new(0, 0, w, h), 0);
        if overlay {
            Self::draw_overlay(svc, g);
        }
    }

    /// Everything of the current view, from a cleared screen.
    fn repaint<S: Services + ?Sized>(&mut self, svc: &S, g: &mut Gfx<'_>, snap: Snap, idle: bool) {
        g.clear(Role::Bg);
        self.art_pending = false;
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

    /// Big art's title band (over the art).
    fn draw_overlay<S: Services + ?Sized>(svc: &S, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        let Some(t) = svc.track() else { return };
        let band = Rect::new(0, h - 40, w, 40);
        let bg = g.c(Role::Bg);
        g.dim(band, bg, 210);
        Self::draw_overlay_progress(svc, g);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(6, band.y + 17, w - 12, t.title, FontId::UiBold, text, Align::Left);
        g.text_in(6, band.y + 33, w - 12, t.artist, FontId::Ui, dim, Align::Left);
    }

    /// The progress line at the top of Big art's band (opaque: redrawn by
    /// itself every second).
    fn draw_overlay_progress<S: Services + ?Sized>(svc: &S, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        let p = svc.player();
        let y = h - 40;
        let prog =
            u16::try_from(u64::from(p.position_ms) * 1000 / u64::from(p.duration_ms.max(1))).unwrap_or(0);
        let acc = g.c(Role::Accent);
        let track = g.pal.mix(Role::Bg, Role::TextDim, 90);
        g.fill(Rect::new(0, y, w, 2), track);
        let pw = i16::try_from(i32::from(w) * i32::from(prog) / 1000).unwrap_or(0);
        g.fill(Rect::new(0, y, pw, 2), acc);
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
                let step = seek_step_ms(cx.svc.setting(id::SEEK_STEP), self.seek_repeats);
                cx.svc.command(PlayerCmd::SeekBy(if k == Key::Next { step } else { -step }));
                return Nav::Stay;
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down) if !self.list_view() => {
                let v = if k == Key::Down { self.view + 1 } else { self.view + VIEWS - 1 };
                self.set_view(cx, v);
                return Nav::Stay;
            }
            Event::Data(d) if d.tag == data::PICTURE => {
                if self.art_pending {
                    self.art_pending = false;
                    match cx.svc.picture_state() {
                        PicState::Shown(_) => self.art_arrived = true,
                        // Dropped: draw (and ask) again.
                        PicState::Pending => self.drawn = None,
                        _ => {}
                    }
                }
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
            // Nothing on this screen shows these (a full redraw would ask
            // for the album art again).
            Event::Service(
                ServiceEvent::Battery { .. }
                | ServiceEvent::Minute
                | ServiceEvent::Volume(_)
                | ServiceEvent::Usb { .. },
            ) => return Nav::Stay,
            Event::Service(_) => {
                self.drawn = None;
                return Nav::Stay;
            }
            _ => {}
        }
        if !self.list_view() {
            return Nav::Stay;
        }
        // Up next / Details: Up/Down move in the list; past either end a
        // fresh press changes the view (the views themselves roll round, so
        // these lists never trap the key: UI-8 leaves them out of the List
        // roll-around setting). A held key stops at the end (05 §3).
        let len = if self.view == V_UP_NEXT {
            p.queue_len.saturating_sub(p.queue_pos)
        } else {
            orpheus_ui::widgets::list::ListSource::len(&Self::details(&*cx.svc, self.lang))
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
            let a = l.handle(&src, ev, LIST_ROWS, cx.list());
            self.list = l;
            a
        } else {
            let rows = Self::details(&*cx.svc, self.lang);
            let mut l = self.list;
            let a = l.handle(&rows, ev, LIST_ROWS, cx.list());
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
        // Same track, art and view since the last full draw: only the clock,
        // the bar, the play state or Big art's overlay changed. The art in
        // the frame buffer is still valid then (it is not asked for again).
        let minor = !idle
            && matches!(self.view, V_STANDARD | V_BIG_ART)
            && self.drawn.is_some_and(|d| {
                Snap { second: snap.second, bar: snap.bar, source: snap.source, overlay: snap.overlay, ..d }
                    == snap
            });
        let arrived = core::mem::take(&mut self.art_arrived);
        if dirty == Dirty::Changed && minor && arrived {
            // The art is in the frame buffer now, over the placeholder (and
            // over Big art's overlay if that was drawn).
            g.mark(self.art_rect(g));
            if self.view == V_BIG_ART && self.drawn.is_some_and(|d| d.overlay) {
                Self::draw_overlay(svc, g);
            }
        }
        if dirty == Dirty::Changed && same {
            return;
        }
        if dirty == Dirty::Changed && minor {
            let was = self.drawn.unwrap_or(snap);
            if self.view == V_STANDARD {
                self.draw_progress_rows(svc, g);
            } else if snap.overlay && !was.overlay {
                Self::draw_overlay(svc, g);
            } else if snap.overlay {
                Self::draw_overlay_progress(svc, g);
            } else if was.overlay {
                // The art under the overlay is needed again.
                self.repaint(svc, g, snap, idle);
                return;
            }
            self.drawn = Some(snap);
            return;
        }
        self.repaint(svc, g, snap, idle);
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
                if !settings::hidden(id::SPEED) {
                    let mut v = StrBuf::<40>::new();
                    if let Some(d) = settings::def(id::SPEED) {
                        settings::format_value(d, svc.setting(id::SPEED), l, &mut v);
                    }
                    out.value(M_SPEED, l.get(Str::Speed), v.as_str());
                }
                out.action(M_EQ, l.get(Str::EqualiserDots));
                out.action(M_SLEEP, l.get(Str::SleepTimerDots));
                if self.view != V_UP_NEXT || self.list.focus == 0 {
                    if svc.lib_now().is_some() {
                        out.action(M_ALBUM, l.get(Str::GoToAlbum));
                        out.action(M_ARTIST, l.get(Str::GoToArtist));
                    }
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
            menu_key: false,
        }
    }

    /// The track (title, artist, playing or paused, its place in the
    /// queue), the times; Up next and Details are lists.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let svc = &*cx.svc;
        let l = self.lang;
        d.title(l.get(Str::NowPlaying));
        let p = svc.player();
        let Some(t) = svc.track().filter(|_| p.queue_len > 0) else {
            d.say(l.get(Str::NothingPlaying));
            d.say(l.get(Str::NothingPlayingAction));
            return;
        };
        if self.view == V_UP_NEXT {
            let src = QueueSource { svc, base: p.queue_pos, len: p.queue_len.saturating_sub(p.queue_pos) };
            d.list(&src, &self.list, "empty");
            return;
        }
        if self.view == V_DETAILS {
            let rows = Self::details(svc, l);
            d.list(&rows, &self.list, "empty");
            return;
        }
        let mut label = StrBuf::<96>::new();
        label.push_str_or_ellipsis(t.title);
        if !t.artist.is_empty() {
            label.push_str(", ");
            label.push_str_or_ellipsis(t.artist);
        }
        let state = match p.source {
            Source::Paused => "paused",
            Source::Playing => "playing",
            _ => "stopped",
        };
        d.focus(orpheus_ui::a11y::Role::Text, label.as_str()).value(state).at(p.queue_pos, p.queue_len);
        d.say(t.album);
        let mut times = StrBuf::<32>::new();
        push_time(&mut times, p.position_ms);
        times.push_str(" of ");
        push_time(&mut times, p.duration_ms);
        d.say(times.as_str());
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
            M_ALBUM | M_ARTIST => {
                if let Some(n) = cx.svc.lib_now() {
                    let list = if mid == M_ALBUM {
                        LibList::AlbumTracks(n.album)
                    } else {
                        LibList::ArtistAlbums(n.artist)
                    };
                    return Nav::Push(LIBRARY, LibraryScreen::arg(list));
                }
            }
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

    /// The preset, the pre-amp and the band chosen with its gain.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let l = self.lang;
        d.title(l.get(Str::Equaliser));
        let preset = cx.svc.setting(id::EQUALISER);
        let mut name = StrBuf::<24>::new();
        if let Some(def) = settings::def(id::EQUALISER) {
            settings::format_value(def, preset, l, &mut name);
        }
        d.say(name.as_str());
        let gains = Self::gains(cx.svc);
        let b = usize::from(self.band % 5);
        let mut label = StrBuf::<16>::new();
        let hz = EQ_BANDS_HZ[b];
        if hz >= 1000 {
            label.push_u32(hz / 1000);
            if hz % 1000 != 0 {
                label.push('.');
                label.push_u32(hz % 1000 / 100);
            }
            label.push_str(" kHz");
        } else {
            label.push_u32(hz);
            label.push_str(" Hz");
        }
        let mut v = StrBuf::<12>::new();
        let g = gains[b];
        if g > 0 {
            v.push('+');
        }
        v.push_i32(i32::from(g));
        v.push_str(" dB");
        d.focus(orpheus_ui::a11y::Role::Slider, label.as_str()).value(v.as_str()).at(u32::from(self.band), 5);
    }
}

#[cfg(test)]
mod tests {
    use super::seek_step_ms;

    /// Settings › Playback › Seek step (VER-13): the first steps of a held
    /// Prev/Next are the setting's; the ladder goes up from there to 30 s.
    #[test]
    fn seek_steps_start_at_the_setting() {
        let steps = |setting| [0u16, 5, 6, 11, 12, 19, 20, 99].map(|r| seek_step_ms(setting, r) / 1000);
        assert_eq!(steps(0), [2, 2, 5, 5, 10, 10, 30, 30], "2 s: the ladder of 05 §7.1");
        assert_eq!(steps(1), [5, 5, 10, 10, 30, 30, 30, 30], "5 s (default)");
        assert_eq!(steps(2), [10, 10, 30, 30, 30, 30, 30, 30], "10 s");
        assert_eq!(steps(3), [30; 8], "30 s");
        // Out of range (a damaged store): the nearest end.
        assert_eq!(seek_step_ms(-1, 0), 2000);
        assert_eq!(seek_step_ms(9, 0), 30_000);
    }
}
