//! Pauses and intonation marks: what punctuation becomes.

/// What caused a pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BreakKind {
    /// Between spelled groups (digits of a phone number, letters of an
    /// address): a short gap without a pitch movement.
    Group = 0,
    /// A quotation mark: no pause, a boundary for emphasis.
    Quote = 1,
    /// `(` `[` `{`: an aside begins (lower and faster in TTS-2).
    ParenOpen = 2,
    /// `)` `]` `}`: the aside ends.
    ParenClose = 3,
    /// `,`
    Comma = 4,
    /// `-`, `–`, `—` between words; file-name separators.
    Dash = 5,
    /// `:`
    Colon = 6,
    /// `;`
    Semicolon = 7,
    /// A line end in line mode (lists, menus).
    Line = 8,
    /// `.` at the end of a sentence.
    Period = 9,
    /// `?`
    Question = 10,
    /// `!`
    Exclamation = 11,
    /// `…`, `...`: trailing off.
    Ellipsis = 12,
    /// A blank line.
    Paragraph = 13,
}

/// All kinds in discriminant order.
pub const KINDS: [BreakKind; 14] = [
    BreakKind::Group,
    BreakKind::Quote,
    BreakKind::ParenOpen,
    BreakKind::ParenClose,
    BreakKind::Comma,
    BreakKind::Dash,
    BreakKind::Colon,
    BreakKind::Semicolon,
    BreakKind::Line,
    BreakKind::Period,
    BreakKind::Question,
    BreakKind::Exclamation,
    BreakKind::Ellipsis,
    BreakKind::Paragraph,
];

/// The pitch movement on the last stressed syllable before a break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Contour {
    /// No movement.
    Level = 0,
    /// A small rise: more is coming (comma, colon, dash).
    Continue = 1,
    /// The fall of a statement (and of a question starting with a
    /// wh-word: "where is it?").
    Fall = 2,
    /// The rise of a yes/no question.
    Rise = 3,
    /// A high fall with emphasis (exclamation).
    Exclaim = 4,
}

impl BreakKind {
    /// From the discriminant.
    pub fn from_u8(v: u8) -> Option<BreakKind> {
        KINDS.get(usize::from(v)).copied()
    }

    /// The pause at 1× in milliseconds (the duration model scales it).
    pub const fn base_ms(self) -> u16 {
        match self {
            BreakKind::Quote => 0,
            BreakKind::Group => 60,
            BreakKind::ParenOpen | BreakKind::ParenClose => 150,
            BreakKind::Comma => 200,
            BreakKind::Dash => 220,
            BreakKind::Colon => 280,
            BreakKind::Semicolon => 300,
            BreakKind::Line => 350,
            BreakKind::Period | BreakKind::Question | BreakKind::Exclamation => 450,
            BreakKind::Ellipsis => 550,
            BreakKind::Paragraph => 750,
        }
    }

    /// The default contour (a question's is chosen by the normaliser).
    pub const fn contour(self) -> Contour {
        match self {
            BreakKind::Group | BreakKind::Quote | BreakKind::ParenOpen | BreakKind::ParenClose => {
                Contour::Level
            }
            BreakKind::Comma | BreakKind::Dash | BreakKind::Colon | BreakKind::Semicolon => Contour::Continue,
            BreakKind::Line | BreakKind::Period | BreakKind::Paragraph => Contour::Fall,
            BreakKind::Question => Contour::Rise,
            BreakKind::Exclamation => Contour::Exclaim,
            BreakKind::Ellipsis => Contour::Level,
        }
    }

    /// Whether it ends a sentence (declination resets after it).
    pub const fn ends_sentence(self) -> bool {
        matches!(
            self,
            BreakKind::Period
                | BreakKind::Question
                | BreakKind::Exclamation
                | BreakKind::Ellipsis
                | BreakKind::Paragraph
                | BreakKind::Line
        )
    }

    /// Whether the word before it is phrase-final (lengthening, contour).
    pub const fn ends_phrase(self) -> bool {
        !matches!(self, BreakKind::Group | BreakKind::Quote)
    }
}

/// A short text mark for a break (goldens, debugging): `,` `.` `?/` (a
/// rising question), `?\` (falling), `!` `...` `:` `;` `-` `(` `)` `"` `|`
/// (group) `/` (line) `¶` (paragraph).
pub const fn mark(kind: BreakKind, contour: Contour) -> &'static str {
    match (kind, contour) {
        (BreakKind::Group, _) => "|",
        (BreakKind::Quote, _) => "\"",
        (BreakKind::ParenOpen, _) => "(",
        (BreakKind::ParenClose, _) => ")",
        (BreakKind::Comma, _) => ",",
        (BreakKind::Dash, _) => "-",
        (BreakKind::Colon, _) => ":",
        (BreakKind::Semicolon, _) => ";",
        (BreakKind::Line, _) => "/",
        (BreakKind::Period, _) => ".",
        (BreakKind::Question, Contour::Fall) => "?\\",
        (BreakKind::Question, _) => "?/",
        (BreakKind::Exclamation, _) => "!",
        (BreakKind::Ellipsis, _) => "...",
        (BreakKind::Paragraph, _) => "¶",
    }
}

impl Contour {
    /// From the discriminant.
    pub fn from_u8(v: u8) -> Option<Contour> {
        [Contour::Level, Contour::Continue, Contour::Fall, Contour::Rise, Contour::Exclaim]
            .get(usize::from(v))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_order() {
        for (i, k) in KINDS.iter().enumerate() {
            assert_eq!(BreakKind::from_u8(i as u8), Some(*k));
        }
        assert_eq!(BreakKind::from_u8(14), None);
        for c in 0..5 {
            assert_eq!(Contour::from_u8(c).map(|x| x as u8), Some(c));
        }
        assert!(BreakKind::Period.base_ms() > BreakKind::Comma.base_ms());
        assert!(BreakKind::Paragraph.base_ms() > BreakKind::Period.base_ms());
        assert!(BreakKind::Question.ends_sentence() && !BreakKind::Comma.ends_sentence());
    }
}
