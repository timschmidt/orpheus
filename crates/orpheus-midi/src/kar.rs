//! Lyrics: karaoke (`.kar`) text events and lyric meta events as timed
//! pieces of text for the UI.
//!
//! Two conventions exist:
//!
//! * **Karaoke files** (`.kar`, "Soft Karaoke"): the syllables are text
//!   events (meta 0x01), usually in a track of their own. A leading `\`
//!   starts a new paragraph (verse), a leading `/` a new line; text
//!   starting with `@` is a header (`@K` the format tag, `@T` title lines,
//!   `@L` language, `@I` information) and not sung. A file is taken as a
//!   karaoke file when it carries `@K` or `@T` headers or the caller says
//!   so (the `.kar` extension).
//! * **Lyric events** (meta 0x05, the SMF standard): one syllable each; a
//!   carriage return or line feed at the end ends the line, at the start
//!   begins a new one; `/` and `\` are honoured as in karaoke files (many
//!   files mix the two).
//!
//! The text is passed on as bytes (karaoke files are mostly Latin-1 or a
//! Windows code page; the UI decides).

use crate::smf::TextKind;

/// Where the lyrics of a file are.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LyricMode {
    /// No lyrics.
    #[default]
    None,
    /// Lyric meta events.
    Lyric,
    /// Karaoke text events.
    Kar,
}

/// A line break before or after a piece.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Break {
    /// None.
    #[default]
    None,
    /// A new line.
    Line,
    /// A new paragraph (verse).
    Paragraph,
}

/// A syllable with its breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece<'a> {
    /// Break before the text.
    pub before: Break,
    /// The text (may be empty: a bare break).
    pub text: &'a [u8],
    /// Break after the text.
    pub after: Break,
}

/// Whether `text` (a text event) is a karaoke header that marks the file
/// as a karaoke file.
#[must_use]
pub fn is_kar_tag(text: &[u8]) -> bool {
    text.starts_with(b"@K") || text.starts_with(b"@T")
}

/// The title from a karaoke `@T` header.
#[must_use]
pub fn kar_title(text: &[u8]) -> Option<&[u8]> {
    text.strip_prefix(b"@T").map(trim)
}

fn trim(mut t: &[u8]) -> &[u8] {
    while let [b' ' | b'\r' | b'\n' | b'\t' | 0, rest @ ..] = t {
        t = rest;
    }
    while let [rest @ .., b' ' | b'\r' | b'\n' | b'\t' | 0] = t {
        t = rest;
    }
    t
}

/// The lyric piece of a text event under `mode`, if it is one.
#[must_use]
pub fn piece(kind: TextKind, text: &[u8], mode: LyricMode) -> Option<Piece<'_>> {
    let mut t = text;
    match (mode, kind) {
        (LyricMode::Kar, TextKind::Text) if !t.starts_with(b"@") => {}
        (LyricMode::Lyric, TextKind::Lyric) => {}
        _ => return None,
    }
    let mut before = Break::None;
    let mut after = Break::None;
    loop {
        match t {
            [b'\\', rest @ ..] => {
                before = Break::Paragraph;
                t = rest;
            }
            [b'/' | b'\r' | b'\n', rest @ ..] => {
                if before == Break::None {
                    before = Break::Line;
                }
                t = rest;
            }
            _ => break,
        }
    }
    while let [rest @ .., b'\r' | b'\n' | 0] = t {
        after = Break::Line;
        t = rest;
    }
    Some(Piece { before, text: t, after })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kar_breaks_and_headers() {
        let p = piece(TextKind::Text, b"\\Morn", LyricMode::Kar).unwrap();
        assert_eq!((p.before, p.text, p.after), (Break::Paragraph, &b"Morn"[..], Break::None));
        let p = piece(TextKind::Text, b"/the ", LyricMode::Kar).unwrap();
        assert_eq!((p.before, p.text), (Break::Line, &b"the "[..]));
        assert!(piece(TextKind::Text, b"@LENGL", LyricMode::Kar).is_none());
        assert!(piece(TextKind::Lyric, b"x", LyricMode::Kar).is_none());
        assert_eq!(kar_title(b"@T Hill Song \r"), Some(&b"Hill Song"[..]));
        assert!(is_kar_tag(b"@KMIDI KARAOKE FILE"));
    }

    #[test]
    fn lyric_events() {
        let p = piece(TextKind::Lyric, b"day\r", LyricMode::Lyric).unwrap();
        assert_eq!((p.before, p.text, p.after), (Break::None, &b"day"[..], Break::Line));
        let p = piece(TextKind::Lyric, b"\nSing", LyricMode::Lyric).unwrap();
        assert_eq!((p.before, p.text), (Break::Line, &b"Sing"[..]));
        assert!(piece(TextKind::Text, b"x", LyricMode::Lyric).is_none());
        assert!(piece(TextKind::Lyric, b"x", LyricMode::None).is_none());
    }
}
