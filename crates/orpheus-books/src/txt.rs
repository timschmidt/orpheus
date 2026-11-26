//! Plain text to paragraphs (05 §7.15).
//!
//! Two kinds of files: hard-wrapped text (lines of at most ~80 characters,
//! paragraphs separated by a blank line or started by an indented line:
//! [`Mode::Join`]), and one paragraph per line ([`Mode::Lines`], files
//! whose head has a line longer than [`LONG_LINE`]). A paragraph of a
//! single short line that looks like a heading ("CHAPTER IV", "Глава 3",
//! "Part Two", a Roman numeral, "* * *") becomes a heading (or a scene
//! break) for the contents.

use crate::norm::{Events, Kind};

/// A line longer than this in the head makes a one-paragraph-per-line
/// file.
pub const LONG_LINE: usize = 120;

/// Characters of a paragraph's first line kept to decide whether it is a
/// heading (longer lines are body text).
const HEAD_CHARS: usize = 48;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Join,
    Lines,
}

/// The mode of a file from its decoded head.
pub fn sniff(head: impl Iterator<Item = char>) -> Mode {
    let mut len = 0usize;
    for c in head {
        if c == '\n' || c == '\r' {
            len = 0;
        } else {
            len += 1;
            if len > LONG_LINE {
                return Mode::Lines;
            }
        }
    }
    Mode::Join
}

/// The plain-text parser: one character at a time.
#[derive(Clone, Copy, Debug)]
pub struct Txt {
    mode: Mode,
    in_para: bool,
    /// Characters of the current line so far (spaces at its start not
    /// counted) and the indentation before them.
    line_len: u16,
    indent: u8,
    after_cr: bool,
    /// The paragraph's first line, while it may still be a heading.
    head: [char; HEAD_CHARS],
    head_n: u8,
    buffering: bool,
    lines: u16,
}

impl Txt {
    #[must_use]
    pub const fn new(mode: Mode) -> Txt {
        Txt {
            mode,
            in_para: false,
            line_len: 0,
            indent: 0,
            after_cr: false,
            head: ['\0'; HEAD_CHARS],
            head_n: 0,
            buffering: false,
            lines: 0,
        }
    }

    fn flush_head(&mut self, ev: &mut impl Events, kind: Kind) {
        if !self.buffering {
            return;
        }
        self.buffering = false;
        ev.para(kind);
        if kind != Kind::Break {
            for &c in self.head.iter().take(usize::from(self.head_n)) {
                ev.text(c);
            }
        }
        self.head_n = 0;
    }

    fn end_para(&mut self, ev: &mut impl Events) {
        if !self.in_para {
            return;
        }
        if self.buffering {
            let n = usize::from(self.head_n);
            let kind = heading_kind(self.head.get(..n).unwrap_or(&[])).unwrap_or(Kind::Body);
            self.flush_head(ev, kind);
        }
        ev.end_para();
        self.in_para = false;
        self.lines = 0;
    }

    fn newline(&mut self, ev: &mut impl Events) {
        if self.line_len == 0 {
            // A blank line ends the paragraph.
            self.end_para(ev);
        } else {
            self.lines = self.lines.saturating_add(1);
            if self.mode == Mode::Lines {
                self.end_para(ev);
            }
        }
        self.line_len = 0;
        self.indent = 0;
    }

    pub fn char(&mut self, c: char, ev: &mut impl Events) {
        if c == '\n' && core::mem::take(&mut self.after_cr) {
            return;
        }
        self.after_cr = c == '\r';
        if c == '\n' || c == '\r' || c == '\u{2029}' {
            self.newline(ev);
            return;
        }
        if self.line_len == 0 && c.is_whitespace() {
            self.indent = self.indent.saturating_add(if c == '\t' { 4 } else { 1 });
            return;
        }
        if c == '\u{0C}' {
            // A form feed: a page break in old texts.
            self.end_para(ev);
            return;
        }
        if self.line_len == 0 {
            // The first character of a line.
            if self.in_para && self.indent >= 2 {
                self.end_para(ev);
            }
            if self.in_para {
                // A wrapped line goes on: not a one-line heading.
                if self.buffering {
                    self.flush_head(ev, Kind::Body);
                }
                ev.text(' ');
            } else {
                self.in_para = true;
                self.buffering = true;
                self.head_n = 0;
            }
        }
        self.line_len = self.line_len.saturating_add(1);
        if self.buffering {
            if let Some(slot) = self.head.get_mut(usize::from(self.head_n)) {
                *slot = c;
                self.head_n += 1;
                return;
            }
            self.flush_head(ev, Kind::Body);
        }
        ev.text(c);
    }

    pub fn finish(&mut self, ev: &mut impl Events) {
        self.end_para(ev);
    }
}

fn lower(c: char) -> char {
    match c {
        'A'..='Z' => char::from(c as u8 + 32),
        'А'..='Я' => char::from_u32(c as u32 + 32).unwrap_or(c),
        'Ё' => 'ё',
        c => c,
    }
}

fn starts_with_word(line: &[char], word: &str) -> bool {
    let mut it = line.iter();
    for w in word.chars() {
        match it.next() {
            Some(&c) if lower(c) == w => {}
            _ => return false,
        }
    }
    // The word ends there (a space, punctuation or the end).
    it.next().is_none_or(|c| !c.is_alphabetic())
}

fn is_roman(s: &[char]) -> bool {
    !s.is_empty() && s.len() <= 7 && s.iter().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C'))
}

/// A paragraph of one short line that names a part or chapter: its
/// heading kind (or [`Kind::Break`] for a scene break).
#[must_use]
pub fn heading_kind(line: &[char]) -> Option<Kind> {
    let end = line.iter().rposition(|c| !c.is_whitespace())? + 1;
    let line = line.get(..end)?;
    if line.len() > 40 {
        return None;
    }
    if line.iter().all(|&c| c == '*' || c == ' ' || c == '#' || c == '~') && line.len() >= 3 {
        return Some(Kind::Break);
    }
    for w in ["part", "book", "volume", "часть", "книга", "том"] {
        if starts_with_word(line, w) {
            return Some(Kind::H1);
        }
    }
    for w in [
        "chapter",
        "prologue",
        "epilogue",
        "preface",
        "introduction",
        "afterword",
        "глава",
        "пролог",
        "эпилог",
        "предисловие",
        "послесловие",
    ] {
        if starts_with_word(line, w) {
            return Some(Kind::H2);
        }
    }
    // "IV", "IV.", "12", "12." alone.
    let core = line.strip_suffix(&['.']).unwrap_or(line);
    if is_roman(core) || (!core.is_empty() && core.len() <= 3 && core.iter().all(char::is_ascii_digit)) {
        return Some(Kind::H2);
    }
    None
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::norm::tests::show;
    use crate::norm::{Emit, Writer};
    use std::string::String;
    use std::vec::Vec;

    pub fn convert(text: &str, mode: Mode) -> String {
        let mut out = Vec::new();
        let mut w = Writer::new();
        let mut p = Txt::new(mode);
        {
            let mut ev = Emit { w: &mut w, out: &mut out };
            for c in text.chars() {
                p.char(c, &mut ev);
            }
            p.finish(&mut ev);
        }
        w.finish(&mut out);
        show(&out)
    }

    #[test]
    fn hard_wrapped_paragraphs() {
        let t = "CHAPTER I.\r\n\r\nThe Period\r\n\r\nIt was the best of times,\r\nit was the worst of times.\r\n\r\n\r\nThere were a king\r\nand a queen.\r\n   Indented new paragraph\r\nwrapped.";
        assert_eq!(sniff(t.chars()), Mode::Join);
        assert_eq!(
            convert(t, Mode::Join),
            "¶2CHAPTER I.¶0The Period¶0It was the best of times, it was the worst of times.¶0There were a king and a queen.¶0Indented new paragraph wrapped."
        );
    }

    #[test]
    fn one_paragraph_per_line() {
        let long = "Анна Павловна кашляла несколько дней, у неё был грипп, как она говорила (грипп был тогда новое слово, употреблявшееся только редкими).";
        let t = std::format!("Глава 1\n{long}\n{long}\n* * *\nIV\n{long}");
        assert_eq!(sniff(t.chars()), Mode::Lines);
        let s = convert(&t, Mode::Lines);
        assert!(s.starts_with("¶2Глава 1¶0Анна"), "{s}");
        assert_eq!(s.matches("¶0Анна").count(), 3);
        assert!(s.contains("¶10¶2IV¶0"), "{s}");
    }

    #[test]
    fn headings() {
        let h = |s: &str| heading_kind(&s.chars().collect::<Vec<_>>());
        assert_eq!(h("CHAPTER XII"), Some(Kind::H2));
        assert_eq!(h("Chapter 3. The Storm"), Some(Kind::H2));
        assert_eq!(h("ЧАСТЬ ВТОРАЯ"), Some(Kind::H1));
        assert_eq!(h("Book the First"), Some(Kind::H1));
        assert_eq!(h("XIV."), Some(Kind::H2));
        assert_eq!(h("12"), Some(Kind::H2));
        assert_eq!(h("* * *"), Some(Kind::Break));
        assert_eq!(h("Chapters of my life were long"), None, "a word, not a prefix");
        assert_eq!(h("Parting is such sweet sorrow"), None);
        assert_eq!(h("I said nothing."), None);
        assert_eq!(h(""), None);
    }

    #[test]
    fn long_first_line_is_body() {
        let line = "x".repeat(HEAD_CHARS + 10);
        let s = convert(&std::format!("Chapter {line}\n\nnext"), Mode::Join);
        assert_eq!(s, std::format!("¶0Chapter {line}¶0next"));
    }
}
