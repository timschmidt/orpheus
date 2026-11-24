//! Text viewer (05 §7.2): word-wrapped, paged with Prev/Next (Up/Down move
//! by a line), remembers the position per file. The file is read through
//! the `TextService` window; text is UTF-8, or Windows-1252-ish Latin-1
//! where it is not valid UTF-8.

use yoton_ui::event::{Event, Key, KeyPhase};
use yoton_ui::font::FontId;
use yoton_ui::geom::Rect;
use yoton_ui::gfx::Gfx;
use yoton_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use yoton_ui::text::{StrBuf, is_cjk, no_line_start};
use yoton_ui::theme::Role;

use crate::browse::FileRef;
use crate::services::{Services, data};
use crate::strings::{Lang, Str};

const PITCH: i16 = 16;
const MARGIN: i16 = 5;
/// Previous page starts kept for Up/Prev (older ones are re-found by
/// scanning back to a line start).
const HISTORY: usize = 24;

/// Decode one character at `i` (UTF-8, else the byte as Latin-1).
fn decode(b: &[u8], i: usize) -> Option<(char, usize)> {
    let first = *b.get(i)?;
    let n = match first {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return Some((char::from(first), 1)),
    };
    match b.get(i..i + n).and_then(|s| core::str::from_utf8(s).ok()).and_then(|s| s.chars().next()) {
        Some(c) => Some((c, n)),
        // Truncated at the end of the window: stop here.
        None if i + n > b.len() => None,
        None => Some((char::from(first), 1)),
    }
}

/// One laid-out line: bytes `[start, end)` of the window, `next` where the
/// following line starts.
#[derive(Clone, Copy, Debug, Default)]
struct Line {
    start: usize,
    end: usize,
    next: usize,
}

/// Lay out the line starting at `start` within `width` pixels.
fn layout(g: &mut Gfx<'_>, b: &[u8], start: usize, width: i16) -> Option<Line> {
    if start >= b.len() {
        return None;
    }
    let mut i = start;
    let mut w = 0i16;
    let mut brk: Option<(usize, usize)> = None; // (line end, next start)
    let mut prev_cjk = false;
    while let Some((c, n)) = decode(b, i) {
        if c == '\n' {
            return Some(Line { start, end: i, next: i + n });
        }
        if c == '\r' {
            i += n;
            continue;
        }
        let cjk = is_cjk(c);
        if c == ' ' {
            brk = Some((i, i + n));
        } else if (cjk || prev_cjk) && i > start && !no_line_start(c) {
            brk = Some((i, i));
        }
        prev_cjk = cjk;
        let mut tmp = [0u8; 4];
        let adv = g.measure(c.encode_utf8(&mut tmp), FontId::Ui);
        if w + adv > width && i > start {
            return Some(match brk {
                Some((end, next)) => Line { start, end, next },
                None => Line { start, end: i, next: i },
            });
        }
        w += adv;
        i += n;
    }
    // End of the window: the rest is one (possibly partial) line.
    Some(Line { start, end: i, next: i })
}

#[derive(Default)]
pub struct TextScreen {
    file: FileRef,
    size: u32,
    top: u32,
    history: [u32; HISTORY],
    depth: u8,
    /// Where the second line and the next page start (from the last draw).
    line2: u32,
    next_page: u32,
    lang: Lang,
    drawn: Option<u32>,
    loading: bool,
}

impl TextScreen {
    fn push_history(&mut self) {
        if usize::from(self.depth) == HISTORY {
            self.history.copy_within(1.., 0);
            self.depth -= 1;
        }
        self.history[usize::from(self.depth)] = self.top;
        self.depth += 1;
    }

    fn go<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, to: u32) {
        if to != self.top && to < self.size {
            self.push_history();
            self.top = to;
            cx.svc.want_text(to);
        }
    }

    fn back<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, pages: bool) {
        if self.depth > 0 {
            self.depth -= 1;
            self.top = self.history[usize::from(self.depth)];
        } else if self.top > 0 {
            // No history (opened mid-file): back to a line start before.
            let (ws, win) = cx.svc.text_window();
            let back = if pages { 400 } else { 40 };
            let target = self.top.saturating_sub(back);
            let mut t = target;
            if t > ws {
                let rel = usize::try_from(t - ws).unwrap_or(0);
                if let Some(nl) = win.get(..rel).and_then(|w| w.iter().rposition(|&b| b == b'\n')) {
                    t = ws + u32::try_from(nl + 1).unwrap_or(0);
                }
            } else {
                t = 0;
            }
            self.top = t.min(self.top.saturating_sub(1));
        }
        cx.svc.want_text(self.top);
    }
}

impl<S: Services + ?Sized> Screen<S> for TextScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        if let Some(e) = cx.svc.entry(arg.0) {
            self.file = e.file;
        }
        self.size = cx.svc.text_open(self.file).unwrap_or(0);
        self.top = cx.svc.text_position(self.file).min(self.size);
        cx.svc.want_text(self.top);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                cx.svc.set_text_position(self.file, self.top);
                return Nav::Pop;
            }
            Event::Key(Key::Next, KeyPhase::Down | KeyPhase::Repeat) => self.go(cx, self.next_page),
            Event::Key(Key::Down, KeyPhase::Down | KeyPhase::Repeat) => self.go(cx, self.line2),
            Event::Key(Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => self.back(cx, true),
            Event::Key(Key::Up, KeyPhase::Down | KeyPhase::Repeat) => self.back(cx, false),
            Event::Data(d) if d.tag == data::TEXT => self.drawn = None,
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn == Some(self.top) {
            return;
        }
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let (ws, win) = cx.svc.text_window();
        let text = g.c(Role::Text);
        let width = w - 2 * MARGIN - 4;
        let lines = h / PITCH;
        let in_window = self.top >= ws && usize::try_from(self.top - ws).is_ok_and(|r| r <= win.len());
        if !in_window || (win.is_empty() && self.size > 0) {
            // Window not there yet: placeholders until Event::Data(TEXT).
            let c = g.pal.mix(Role::Bg, Role::TextDim, 70);
            for i in 0..lines {
                let lw = 60 + (i * 23) % 50;
                g.fill_round(Rect::new(MARGIN, i * PITCH + 6, lw, 7), 3, c);
            }
            self.loading = true;
            return;
        }
        self.loading = false;
        let mut pos = usize::try_from(self.top - ws).unwrap_or(0);
        self.line2 = self.top;
        self.next_page = self.top;
        // Lines that need the 16 px fallback font get 2 px more pitch.
        let mut y = 0;
        for i in 0..lines {
            let Some(line) = layout(g, win, pos, width) else { break };
            let mut s = StrBuf::<192>::new();
            let mut j = line.start;
            while j < line.end {
                let Some((c, n)) = decode(win, j) else { break };
                if c != '\r' {
                    s.push(c);
                }
                j += n;
            }
            let pitch = if g.needs_fallback(s.as_str(), FontId::Ui) { PITCH + 2 } else { PITCH };
            if y + pitch > h + 2 {
                break;
            }
            g.text(MARGIN, y + 13, s.as_str(), FontId::Ui, text);
            y += pitch;
            pos = line.next;
            let at = ws + u32::try_from(pos).unwrap_or(0);
            if i == 0 {
                self.line2 = at;
            }
            self.next_page = at;
        }
        // Position bar on the right edge.
        if self.size > 0 {
            let track = yoton_ui::geom::Rect::new(w - 3, 2, 2, h - 4);
            let tc = g.pal.mix(Role::Bg, Role::TextDim, 60);
            g.fill_round(track, 1, tc);
            let th = i64::from(track.h);
            let y0 = th * i64::from(self.top) / i64::from(self.size);
            let y1 = (th * i64::from(self.next_page) / i64::from(self.size)).max(y0 + 6);
            let thumb = Rect::new(
                track.x,
                track.y + i16::try_from(y0).unwrap_or(0),
                2,
                i16::try_from(y1 - y0).unwrap_or(6).min(track.h),
            );
            let c = g.c(Role::TextDim);
            g.fill_round(thumb, 1, c);
        }
        self.drawn = Some(self.top);
    }

    fn wants(&self) -> Wants {
        Wants { strip: StripUse::Content, title: Some(self.lang.get(Str::KindText)), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        let mut r = Resume { top: self.top, ..Resume::default() };
        r.data[..8].copy_from_slice(&self.file.to_bytes());
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.file = FileRef::from_bytes(&r.data[..8]);
        self.size = cx.svc.text_open(self.file).unwrap_or(0);
        self.top = r.top.min(self.size);
        cx.svc.want_text(self.top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_falls_back_to_latin1() {
        assert_eq!(decode(b"a", 0), Some(('a', 1)));
        assert_eq!(decode("ж".as_bytes(), 0), Some(('ж', 2)));
        assert_eq!(decode(b"\xE9t\xE9", 0), Some(('é', 1)));
        assert_eq!(decode(&"ж".as_bytes()[..1], 0), None, "cut at the window end");
    }
}
