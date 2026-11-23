//! `Dialog`: title, up to three lines of text, one to three buttons;
//! destructive dialogs focus the safe button (05 §2.6, §5.2).

use crate::event::{DIALOG_CANCELLED, Event, Key, KeyPhase};
use crate::font::FontId;
use crate::geom::Rect;
use crate::gfx::{Align, Gfx, SCREEN_H, SCREEN_W, STATUS_BAR};
use crate::text::StrBuf;
use crate::theme::Role;

/// What to show. Built by a screen and handed to `cx.ui.dialog()`.
#[derive(Clone, Copy, Debug)]
pub struct DialogSpec {
    /// The screen's tag, returned in `UiEvent::Dialog`.
    pub id: u16,
    pub title: &'static str,
    pub text: StrBuf<96>,
    pub buttons: [&'static str; 3],
    pub n_buttons: u8,
    /// Button focused when the dialog opens (the safe one).
    pub focus: u8,
    /// Index of a destructive button, drawn in the warning colour.
    pub danger: Option<u8>,
}

impl DialogSpec {
    /// One button ("OK").
    #[must_use]
    pub fn info(id: u16, title: &'static str, text: &str, ok: &'static str) -> DialogSpec {
        DialogSpec {
            id,
            title,
            text: StrBuf::from(text),
            buttons: [ok, "", ""],
            n_buttons: 1,
            focus: 0,
            danger: None,
        }
    }

    /// Two buttons, the second (affirmative) focused.
    #[must_use]
    pub fn confirm(
        id: u16,
        title: &'static str,
        text: &str,
        no: &'static str,
        yes: &'static str,
    ) -> DialogSpec {
        DialogSpec {
            id,
            title,
            text: StrBuf::from(text),
            buttons: [no, yes, ""],
            n_buttons: 2,
            focus: 1,
            danger: None,
        }
    }

    /// Two buttons; the destructive one (index 1) is red and the safe one
    /// (index 0) is focused.
    #[must_use]
    pub fn destructive(
        id: u16,
        title: &'static str,
        text: &str,
        safe: &'static str,
        act: &'static str,
    ) -> DialogSpec {
        DialogSpec {
            id,
            title,
            text: StrBuf::from(text),
            buttons: [safe, act, ""],
            n_buttons: 2,
            focus: 0,
            danger: Some(1),
        }
    }

    /// Three buttons, the first focused.
    #[must_use]
    pub fn three(id: u16, title: &'static str, text: &str, b: [&'static str; 3]) -> DialogSpec {
        DialogSpec { id, title, text: StrBuf::from(text), buttons: b, n_buttons: 3, focus: 0, danger: None }
    }
}

/// An open dialog.
pub struct Dialog {
    pub spec: DialogSpec,
    focus: u8,
    drawn_focus: Option<u8>,
    frame_drawn: bool,
    buttons_y: i16,
}

/// Result of a key on an open dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogResult {
    None,
    /// Closed with this button, or [`DIALOG_CANCELLED`].
    Done(u8),
}

const MAX_LINES: usize = 3;
const CARD_W: i16 = 124;
const PAD: i16 = 6;

impl Dialog {
    #[must_use]
    pub fn new(spec: DialogSpec) -> Dialog {
        let focus = spec.focus.min(spec.n_buttons.saturating_sub(1));
        Dialog { spec, focus, drawn_focus: None, frame_drawn: false, buttons_y: 0 }
    }

    pub fn invalidate(&mut self) {
        self.frame_drawn = false;
        self.drawn_focus = None;
    }

    #[must_use]
    pub fn focus(&self) -> u8 {
        self.focus
    }

    pub fn handle(&mut self, ev: &Event) -> DialogResult {
        let Event::Key(k, p) = *ev else { return DialogResult::None };
        let n = self.spec.n_buttons.max(1);
        match (k, p) {
            (Key::Prev | Key::Up, KeyPhase::Down | KeyPhase::Repeat) => {
                self.focus = self.focus.saturating_sub(1);
            }
            (Key::Next | Key::Down, KeyPhase::Down | KeyPhase::Repeat) => {
                self.focus = (self.focus + 1).min(n - 1);
            }
            (Key::Ok, KeyPhase::Up { held: false }) => return DialogResult::Done(self.focus),
            (Key::Back, KeyPhase::Up { held: false }) => return DialogResult::Done(DIALOG_CANCELLED),
            _ => {}
        }
        DialogResult::None
    }

    /// Draw (panel coordinates). The first draw lays out the card; later
    /// draws only repaint the buttons when focus moves.
    pub fn draw(&mut self, g: &mut Gfx<'_>) {
        let x0 = (SCREEN_W - CARD_W) / 2;
        let inner = CARD_W - 2 * PAD;
        if !self.frame_drawn {
            let text = self.spec.text;
            let s = text.as_str();
            let total = g.wrap(s, FontId::Ui, inner, |_, _| {});
            let lines = total.min(MAX_LINES);
            let lines_i = i16::try_from(lines).unwrap_or(0);
            let title_h = if self.spec.title.is_empty() { 0 } else { 18 };
            let pitch = g.line_pitch(s, FontId::Ui);
            let text_h = if lines == 0 { 0 } else { pitch * lines_i };
            let h = 9 + title_h + text_h + 9 + 19 + 7;
            let region = SCREEN_H - STATUS_BAR.h;
            let y0 = STATUS_BAR.h + (region - h) / 2;
            let surf = g.c(Role::Surface);
            g.fill_round(Rect::new(x0, y0, CARD_W, h), 8, surf);
            let mut y = y0 + 9;
            if !self.spec.title.is_empty() {
                let c = g.c(Role::Text);
                g.text_in(x0 + PAD, y + 10, inner, self.spec.title, FontId::UiBold, c, Align::Center);
                y += title_h;
            }
            let c = g.c(if self.spec.title.is_empty() { Role::Text } else { Role::TextDim });
            let mut i = 0usize;
            let base = s.as_ptr() as usize;
            g.wrap(s, FontId::Ui, inner, |g, line| {
                if i < MAX_LINES {
                    let ly = y + pitch - 4 + pitch * i16::try_from(i).unwrap_or(0);
                    if i == MAX_LINES - 1 && total > MAX_LINES {
                        // More text follows: cut the third line with an ellipsis.
                        let rest = s.get(line.as_ptr() as usize - base..).unwrap_or(line);
                        g.text_in(x0 + PAD, ly, inner, rest, FontId::Ui, c, Align::Left);
                    } else {
                        g.text_in(x0 + PAD, ly, inner, line, FontId::Ui, c, Align::Center);
                    }
                }
                i += 1;
            });
            self.buttons_y = y + text_h + 9;
            self.frame_drawn = true;
            self.drawn_focus = None;
        }
        if self.drawn_focus != Some(self.focus) {
            self.draw_buttons(g, x0);
            self.drawn_focus = Some(self.focus);
        }
    }

    fn draw_buttons(&mut self, g: &mut Gfx<'_>, x0: i16) {
        let n = i16::from(self.spec.n_buttons.clamp(1, 3));
        let gap = 4;
        let total = CARD_W - 2 * PAD;
        let bw = (total - gap * (n - 1)) / n;
        let y = self.buttons_y;
        for i in 0..n {
            let bx = x0 + PAD + i * (bw + gap);
            let r = Rect::new(bx, y, bw, 19);
            let idx = u8::try_from(i).unwrap_or(0);
            let focused = idx == self.focus;
            let danger = self.spec.danger == Some(idx);
            g.fill_role(r, Role::Surface);
            let (bg, fg) = if focused {
                (g.c(if danger { Role::Warning } else { Role::Accent }), g.c(Role::OnAccent))
            } else {
                let fg = if danger { g.c(Role::Warning) } else { g.c(Role::Text) };
                (g.pal.mix(Role::Surface, Role::Text, 28), fg)
            };
            g.fill_round(r, 6, bg);
            let label = self.spec.buttons.get(usize::from(idx)).copied().unwrap_or("");
            // Bold only if it fits; a label never loses letters to the weight.
            let font = if focused && g.measure(label, FontId::UiBold) <= bw - 4 {
                FontId::UiBold
            } else {
                FontId::Ui
            };
            g.text_in(bx + 2, y + 14, bw - 4, label, font, fg, Align::Center);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_focuses_safe() {
        let mut d = Dialog::new(DialogSpec::destructive(7, "Delete?", "x", "Cancel", "Delete"));
        assert_eq!(d.focus(), 0);
        d.handle(&Event::Key(Key::Next, KeyPhase::Down));
        assert_eq!(d.focus(), 1);
        assert_eq!(d.handle(&Event::Key(Key::Ok, KeyPhase::Up { held: false })), DialogResult::Done(1));
        assert_eq!(
            d.handle(&Event::Key(Key::Back, KeyPhase::Up { held: false })),
            DialogResult::Done(DIALOG_CANCELLED)
        );
    }
}
