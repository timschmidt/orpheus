//! Status bar and bottom strip (05 §4).

use crate::event::{BtState, Source};
use crate::font::FontId;
use crate::geom::Rect;
use crate::gfx::{Align, Gfx, STRIP};
use crate::icons::{self, Icon};
use crate::text::StrBuf;
use crate::theme::Role;

/// Everything the status bar shows. Compared with the last drawn value to
/// decide whether to redraw.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    /// Hours and minutes; `None` shows the screen title instead.
    pub clock: Option<(u8, u8)>,
    pub battery: u8,
    pub charging: bool,
    pub bt: BtState,
    pub source: Source,
    pub locked: bool,
    pub sleep_timer: bool,
    pub alarm: bool,
    pub activity: bool,
}

/// Draw the status bar into the current area (128 × 14).
/// Icons are placed right to left in priority order (battery, charging,
/// Bluetooth, source, lock, sleep timer, alarm, activity); those that would
/// collide with the clock are left out. A title (clock off) gets at least
/// half the bar.
pub fn draw_status_bar(g: &mut Gfx<'_>, s: &Status, title: Option<&str>) {
    let (w, h) = g.size();
    g.fill_role(Rect::new(0, 0, w, h), Role::Surface);
    let text = g.c(Role::Text);
    let mut clock = StrBuf::<8>::new();
    let label = match s.clock {
        Some((hh, mm)) => {
            clock.push_u32_padded(u32::from(hh), 2);
            clock.push(':');
            clock.push_u32_padded(u32::from(mm), 2);
            Some(clock.as_str())
        }
        None => title,
    };
    let label_w = match (label, s.clock) {
        (Some(l), Some(_)) => g.measure(l, FontId::UiBold),
        (Some(l), None) => g.measure(l, FontId::UiBold).min(w / 2),
        (None, _) => 0,
    };
    let reserved = 5 + label_w + 4;
    let mut x = w - 4;
    // Battery: 18 x 9 outline with a nub, five 2 px segments.
    x -= 20;
    draw_battery(g, x, 3, s.battery, s.charging);
    let dim = g.c(Role::TextDim);
    let warn = g.c(Role::Warning);
    let good = g.c(Role::Good);
    let acc = g.c(Role::Accent);
    let mut wanted: [Option<(&Icon, u16)>; 7] = [None; 7];
    if s.charging {
        wanted[0] = Some((&icons::BOLT, good));
    }
    wanted[1] = match s.bt {
        BtState::Off => None,
        BtState::On => Some((&icons::BT_ON, dim)),
        BtState::Connected => Some((&icons::BT_CONNECTED, text)),
        BtState::Streaming => Some((&icons::BT_STREAMING, text)),
    };
    wanted[2] = match s.source {
        Source::Idle => None,
        Source::Playing => Some((&icons::PLAY, text)),
        Source::Paused => Some((&icons::PAUSE, text)),
        Source::Recording => Some((&icons::RECORD, warn)),
        Source::Radio => Some((&icons::FM, text)),
    };
    if s.locked {
        wanted[3] = Some((&icons::LOCK, text));
    }
    if s.sleep_timer {
        wanted[4] = Some((&icons::MOON, text));
    }
    if s.alarm {
        wanted[5] = Some((&icons::BELL, text));
    }
    if s.activity {
        wanted[6] = Some((&icons::DOT, acc));
    }
    for (ic, c) in wanted.into_iter().flatten() {
        let nx = x - i16::from(ic.w) - 3;
        if nx < reserved {
            break;
        }
        x = nx;
        g.icon(x, (h - i16::from(ic.h)) / 2, ic, c);
    }
    if let Some(l) = label {
        g.text_in(5, 11, x - 4 - 5, l, FontId::UiBold, text, Align::Left);
    }
}

fn draw_battery(g: &mut Gfx<'_>, x: i16, y: i16, pct: u8, charging: bool) {
    let low = pct < 10 && !charging;
    let outline = if low { g.c(Role::Warning) } else { g.c(Role::Text) };
    let body = Rect::new(x, y, 18, 9);
    g.stroke_round(body, 2, outline);
    g.fill(Rect::new(x + 18, y + 3, 2, 3), outline);
    let lit = if pct == 0 { 0 } else { (i16::from(pct.min(100)) + 19) / 20 };
    let seg = if charging {
        g.c(Role::Good)
    } else if low {
        g.c(Role::Warning)
    } else {
        g.c(Role::Text)
    };
    for i in 0..5 {
        let r = Rect::new(x + 2 + i * 3, y + 2, 2, 5);
        if i < lit {
            g.fill(r, seg);
        }
    }
}

/// The mini-player strip contents (05 §4 "Strip").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MiniPlayer<'a> {
    pub title: &'a str,
    pub playing: bool,
    /// 0..=1000.
    pub progress: u16,
}

/// Draw a hint line into the current area (128 × 18).
pub fn draw_hint(g: &mut Gfx<'_>, hint: &str) {
    let (w, h) = g.size();
    g.fill_role(Rect::new(0, 0, w, h), Role::Bg);
    let rule = g.pal.mix(Role::Bg, Role::TextDim, 60);
    g.hline(0, 0, w, rule);
    let c = g.c(Role::TextDim);
    g.text_in(4, 13, w - 8, hint, FontId::Ui, c, Align::Center);
}

/// Blank strip.
pub fn draw_blank_strip(g: &mut Gfx<'_>) {
    g.clear(Role::Bg);
}

const MINI_X: i16 = 18;

/// Pixels by which a mini-player title overflows its space.
pub fn mini_player_overflow(g: &mut Gfx<'_>, title: &str) -> i16 {
    let avail = STRIP.w - MINI_X - 4;
    (g.measure(title, FontId::Ui) - avail).max(0)
}

/// Draw the mini-player into the current area (128 × 18), the title
/// scrolled left by `scroll` pixels (see `marquee_offset`).
pub fn draw_mini_player(g: &mut Gfx<'_>, m: &MiniPlayer<'_>, scroll: i16) {
    let (w, h) = g.size();
    g.fill_role(Rect::new(0, 0, w, h), Role::Surface);
    let track = g.pal.mix(Role::Surface, Role::TextDim, 70);
    g.fill(Rect::new(0, 0, w, 2), track);
    let pw = i16::try_from(i32::from(w) * i32::from(m.progress.min(1000)) / 1000).unwrap_or(0);
    let acc = g.c(Role::Accent);
    g.fill(Rect::new(0, 0, pw, 2), acc);
    let text = g.c(Role::Text);
    g.icon(4, 6, if m.playing { &icons::PLAY } else { &icons::PAUSE }, text);
    let avail = w - MINI_X - 4;
    if scroll > 0 {
        g.with_clip(Rect::new(MINI_X, 2, avail, h - 2), |g| {
            g.text(MINI_X - scroll, 14, m.title, FontId::Ui, text)
        });
    } else {
        g.text_in(MINI_X, 14, avail, m.title, FontId::Ui, text, Align::Left);
    }
}
