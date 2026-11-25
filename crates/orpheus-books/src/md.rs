//! Markdown, a reduced set (05 §7.15): ATX headings (`#`–`######`),
//! paragraphs (lines joined until a blank line), block quotes (`>`),
//! bullet and numbered list items, fenced code (```` ``` ````, `~~~`; each
//! line a preformatted paragraph), thematic breaks (`---`, `***`, `___`),
//! emphasis (`*`/`_` italic, `**`/`__` bold, `***` both), inline code
//! (shown plain), links and images (their text), backslash escapes, HTML
//! tags dropped, a YAML front matter block skipped. Not handled: setext
//! headings, tables (shown as text), nested lists (flattened), reference
//! definitions (shown as text).

use crate::norm::{BOLD, Events, ITALIC, Kind};

/// Characters of a line's start looked at before deciding what the line
/// is (a thematic break needs the whole line).
const PREFIX: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Link {
    None,
    /// After `]`: is a `(` or `[` next?
    Close,
    /// Skipping `(url)` or `[ref]` (the closing byte).
    Skip(char),
}

/// The Markdown parser: one character at a time.
#[derive(Clone, Copy, Debug)]
pub struct Md {
    /// The current paragraph's kind, `None` outside one.
    para: Option<Kind>,
    fence: Option<char>,
    front: u8,
    lines: u32,
    pre: [char; PREFIX],
    pre_n: u8,
    /// Still collecting the line's start.
    deciding: bool,
    after_cr: bool,
    // Inline state.
    bold: bool,
    italic: bool,
    code: bool,
    run: char,
    run_n: u8,
    prev: char,
    escape: bool,
    bang: bool,
    tag: bool,
    link: Link,
}

impl Default for Md {
    fn default() -> Self {
        Md::new()
    }
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\0'
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || matches!(c, '—' | '–' | '«' | '»' | '“' | '”' | '‘' | '’' | '…')
}

impl Md {
    #[must_use]
    pub const fn new() -> Md {
        Md {
            para: None,
            fence: None,
            front: 0,
            lines: 0,
            pre: ['\0'; PREFIX],
            pre_n: 0,
            deciding: true,
            after_cr: false,
            bold: false,
            italic: false,
            code: false,
            run: '\0',
            run_n: 0,
            prev: '\0',
            escape: false,
            bang: false,
            tag: false,
            link: Link::None,
        }
    }

    fn flags(&self) -> u8 {
        (if self.bold { BOLD } else { 0 }) | (if self.italic { ITALIC } else { 0 })
    }

    fn reset_inline(&mut self) {
        self.bold = false;
        self.italic = false;
        self.code = false;
        self.run_n = 0;
        self.prev = '\0';
        self.escape = false;
        self.bang = false;
        self.tag = false;
        self.link = Link::None;
    }

    fn end_para(&mut self, ev: &mut impl Events) {
        self.flush_run('\0', ev);
        if self.bang {
            self.bang = false;
            ev.text('!');
        }
        if self.para.take().is_some() {
            ev.end_para();
        }
        self.reset_inline();
    }

    fn start(&mut self, kind: Kind, ev: &mut impl Events) {
        self.end_para(ev);
        self.para = Some(kind);
        ev.para(kind);
    }

    /// Resolve a pending run of `*` or `_` now that `next` follows it.
    fn flush_run(&mut self, next: char, ev: &mut impl Events) {
        let n = core::mem::take(&mut self.run_n);
        if n == 0 {
            return;
        }
        let d = self.run;
        let prev = self.prev;
        let opens = !is_space(next) && (d == '*' || is_space(prev) || is_punct(prev));
        let closes = !is_space(prev) && (d == '*' || is_space(next) || is_punct(next));
        let (want_b, want_i) = match n {
            1 => (false, true),
            2 => (true, false),
            _ => (true, true),
        };
        let mut changed = false;
        if want_b && ((self.bold && closes) || (!self.bold && opens)) {
            self.bold = !self.bold;
            changed = true;
        }
        if want_i && ((self.italic && closes) || (!self.italic && opens)) {
            self.italic = !self.italic;
            changed = true;
        }
        if changed {
            ev.style(self.flags());
        } else {
            for _ in 0..n {
                ev.text(d);
            }
        }
        self.prev = d;
    }

    fn inline(&mut self, c: char, ev: &mut impl Events) {
        if self.code {
            if c == '`' {
                self.code = false;
            } else {
                ev.text(c);
            }
            self.prev = c;
            return;
        }
        if self.tag {
            if c == '>' {
                self.tag = false;
            }
            return;
        }
        match self.link {
            Link::Skip(end) => {
                if c == end {
                    self.link = Link::None;
                }
                return;
            }
            Link::Close => {
                self.link = Link::None;
                if c == '(' {
                    self.link = Link::Skip(')');
                    return;
                }
                if c == '[' {
                    self.link = Link::Skip(']');
                    return;
                }
            }
            Link::None => {}
        }
        if self.escape {
            self.escape = false;
            self.flush_run(c, ev);
            ev.text(c);
            self.prev = c;
            return;
        }
        if (c == '*' || c == '_') && (self.run_n == 0 || self.run == c) {
            self.run = c;
            self.run_n = self.run_n.saturating_add(1);
            return;
        }
        self.flush_run(c, ev);
        if core::mem::take(&mut self.bang) && c != '[' {
            ev.text('!');
        }
        match c {
            '\\' => self.escape = true,
            '`' => self.code = true,
            '!' => self.bang = true,
            '[' => {}
            ']' => self.link = Link::Close,
            '<' => self.tag = true,
            _ => ev.text(c),
        }
        self.prev = c;
    }

    /// The collected start of a line is complete enough (or the line
    /// ended): decide what the line is, then hand on its characters.
    fn decide(&mut self, eol: bool, ev: &mut impl Events) {
        self.deciding = false;
        let n = usize::from(self.pre_n);
        let mut line = [' '; PREFIX];
        line[..n].copy_from_slice(&self.pre[..n]);
        let line = &line[..n];
        let indent = line.iter().take_while(|c| is_space(**c)).count();
        let body = &line[indent..];
        let first_line = self.lines == 0;
        // Front matter: `---` as the very first line, until `---` or `...`.
        if first_line && eol && is_rule_of(body, '-') && body.len() == 3 {
            self.front = 1;
            return;
        }
        if self.front == 1 {
            if eol && (is_rule_of(body, '-') || is_rule_of(body, '.')) {
                self.front = 2;
            }
            return;
        }
        // Fences.
        let fence = body.first().copied().filter(|&f| (f == '`' || f == '~') && body.starts_with(&[f, f, f]));
        if let Some(open) = self.fence {
            if fence == Some(open) {
                self.fence = None;
                self.end_para(ev);
                return;
            }
            // A code line: its own preformatted paragraph, spaces kept.
            self.start(Kind::Pre, ev);
            self.code = true;
            for &c in line {
                ev.text(if c == '\t' { ' ' } else { c });
            }
            return;
        }
        if let Some(f) = fence {
            self.end_para(ev);
            self.fence = Some(f);
            // The rest of the line (an info string) is dropped.
            self.code = false;
            self.tag = true;
            self.prev = '\0';
            return;
        }
        if body.is_empty() {
            if eol {
                self.end_para(ev);
            }
            return;
        }
        // Thematic break.
        if eol && body.len() >= 3 && (is_rule_of(body, '-') || is_rule_of(body, '*') || is_rule_of(body, '_'))
        {
            self.end_para(ev);
            ev.para(Kind::Break);
            return;
        }
        // ATX heading.
        let hashes = body.iter().take_while(|&&c| c == '#').count();
        if (1..=6).contains(&hashes) && body.get(hashes).is_none_or(|c| is_space(*c)) {
            self.start(Kind::heading(u8::try_from(hashes).unwrap_or(3)), ev);
            self.rest(&body[hashes..], ev);
            return;
        }
        // Block quote.
        if body[0] == '>' {
            if self.para != Some(Kind::Quote) {
                self.start(Kind::Quote, ev);
            } else {
                self.continuation(ev);
            }
            self.rest(&body[1..], ev);
            return;
        }
        // List items.
        if matches!(body[0], '-' | '*' | '+') && body.get(1).is_some_and(|c| is_space(*c)) {
            self.start(Kind::Item, ev);
            ev.text('•');
            ev.text(' ');
            self.rest(&body[2..], ev);
            return;
        }
        let digits = body.iter().take_while(|c| c.is_ascii_digit()).count();
        if (1..=3).contains(&digits)
            && matches!(body.get(digits), Some('.' | ')'))
            && body.get(digits + 1).is_none_or(|c| is_space(*c))
        {
            self.start(Kind::Item, ev);
            for &c in &body[..digits] {
                ev.text(c);
            }
            ev.text('.');
            ev.text(' ');
            self.rest(&body[(digits + 1).min(body.len())..], ev);
            return;
        }
        if self.para.is_some() {
            self.continuation(ev);
        } else {
            self.start(Kind::Body, ev);
        }
        self.rest(body, ev);
    }

    /// A line that continues the paragraph: a space between the lines.
    fn continuation(&mut self, ev: &mut impl Events) {
        self.flush_run(' ', ev);
        ev.text(' ');
        self.prev = ' ';
    }

    fn rest(&mut self, chars: &[char], ev: &mut impl Events) {
        for &c in chars {
            self.inline(c, ev);
        }
    }

    pub fn char(&mut self, c: char, ev: &mut impl Events) {
        if c == '\n' && core::mem::take(&mut self.after_cr) {
            return;
        }
        self.after_cr = c == '\r';
        if c == '\n' || c == '\r' {
            if self.deciding {
                self.decide(true, ev);
            }
            // Code spans and tags do not cross lines; a run is resolved.
            if self.fence.is_some() {
                self.end_para(ev);
            } else {
                self.flush_run(' ', ev);
                self.code = false;
                self.tag = false;
            }
            self.lines = self.lines.saturating_add(1);
            self.deciding = true;
            self.pre_n = 0;
            return;
        }
        if self.deciding {
            if let Some(slot) = self.pre.get_mut(usize::from(self.pre_n)) {
                *slot = c;
                self.pre_n += 1;
                return;
            }
            self.decide(false, ev);
        }
        if self.front == 1 {
            return;
        }
        if self.fence.is_some() {
            if self.para == Some(Kind::Pre) {
                ev.text(if c == '\t' { ' ' } else { c });
            }
            return;
        }
        self.inline(c, ev);
    }

    pub fn finish(&mut self, ev: &mut impl Events) {
        if self.deciding && self.pre_n > 0 {
            self.decide(true, ev);
        }
        self.end_para(ev);
    }
}

/// `line` is `ch` three or more times, spaces between allowed.
fn is_rule_of(line: &[char], ch: char) -> bool {
    let mut n = 0;
    for &c in line {
        if c == ch {
            n += 1;
        } else if !is_space(c) {
            return false;
        }
    }
    n >= 3
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::norm::tests::show;
    use crate::norm::{Emit, Writer};
    use std::string::String;
    use std::vec::Vec;

    fn convert(text: &str) -> String {
        let mut out = Vec::new();
        let mut w = Writer::new();
        let mut p = Md::new();
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
    fn blocks() {
        let t = "---\ntitle: Notes\n---\n# The Title\n\nSome *emphasis*, **bold**\nand `code_x*`.\n\n> quoted\n> more\n\n- one\n- two\n3. three\n\n---\n## Sub\n```rust\nlet  x = 1;\n```\nend";
        assert_eq!(
            convert(t),
            "¶1The Title¶0Some {i}emphasis{}, {b}bold{} and code_x*.¶5quoted more¶8• one¶8• two¶83. three¶10¶2Sub¶7let x = 1;¶0end"
        );
    }

    #[test]
    fn inline() {
        assert_eq!(convert("a_b_c and _it_ and __b__"), "¶0a_b_c and {i}it{} and {b}b");
        assert_eq!(convert("***both*** \\*not\\*"), "¶0{bi}both{} *not*");
        assert_eq!(convert("see [the site](http://x.y/z_(1)) or ![pic](p.png)"), "¶0see the site) or pic");
        assert_eq!(convert("[ref][1] <b>html</b> 2 * 3"), "¶0ref html 2 * 3");
        assert_eq!(convert("*unclosed\n\nnext"), "¶0{i}unclosed¶0next");
        assert_eq!(convert("wow!"), "¶0wow!");
    }

    #[test]
    fn headings_need_a_space() {
        assert_eq!(convert("#hashtag\n####### seven"), "¶0#hashtag ####### seven");
        assert_eq!(convert("###### six"), "¶3six");
    }

    #[test]
    fn long_prefix_lines() {
        let long = "word ".repeat(20);
        assert_eq!(convert(&long), std::format!("¶0{}", long.trim_end()));
    }
}
