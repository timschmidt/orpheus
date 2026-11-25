//! Numbers as English words (US style: no "and" after "hundred"):
//! cardinals up to 15 digits, ordinals, years, digit strings, decimals.

use crate::normalise::Sink;

/// Number words, 0–19.
pub const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

/// Tens, 20–90 (index 2–9).
pub const TENS: [&str; 10] =
    ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// Scale words for groups of three digits.
pub const SCALES: [&str; 5] = ["", "thousand", "million", "billion", "trillion"];

/// Ordinal forms of the words above that end a number.
pub const ORDINALS: [(&str, &str); 33] = [
    ("zero", "zeroth"),
    ("one", "first"),
    ("two", "second"),
    ("three", "third"),
    ("four", "fourth"),
    ("five", "fifth"),
    ("six", "sixth"),
    ("seven", "seventh"),
    ("eight", "eighth"),
    ("nine", "ninth"),
    ("ten", "tenth"),
    ("eleven", "eleventh"),
    ("twelve", "twelfth"),
    ("thirteen", "thirteenth"),
    ("fourteen", "fourteenth"),
    ("fifteen", "fifteenth"),
    ("sixteen", "sixteenth"),
    ("seventeen", "seventeenth"),
    ("eighteen", "eighteenth"),
    ("nineteen", "nineteenth"),
    ("twenty", "twentieth"),
    ("thirty", "thirtieth"),
    ("forty", "fortieth"),
    ("fifty", "fiftieth"),
    ("sixty", "sixtieth"),
    ("seventy", "seventieth"),
    ("eighty", "eightieth"),
    ("ninety", "ninetieth"),
    ("hundred", "hundredth"),
    ("thousand", "thousandth"),
    ("million", "millionth"),
    ("billion", "billionth"),
    ("trillion", "trillionth"),
];

/// Plural forms for decades ("the nineteen nineties", "the two thousands").
pub const DECADES: [(&str, &str); 10] = [
    ("ten", "tens"),
    ("twenty", "twenties"),
    ("thirty", "thirties"),
    ("forty", "forties"),
    ("fifty", "fifties"),
    ("sixty", "sixties"),
    ("seventy", "seventies"),
    ("eighty", "eighties"),
    ("ninety", "nineties"),
    ("thousand", "thousands"),
];

/// Largest cardinal read as a number (15 digits); longer digit strings are
/// read digit by digit.
pub const MAX_CARDINAL: u64 = 999_999_999_999_999;

/// How the last word of a number is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// "twenty-one".
    Cardinal,
    /// "twenty-first".
    Ordinal,
    /// "twenties" (decades).
    Plural,
    /// "fifths" (fractions).
    OrdinalPlural,
}

/// Holds back the last word so it can be changed to its ordinal or plural.
struct Last<'a, S: Sink> {
    sink: &'a mut S,
    held: Option<&'static str>,
}

impl<S: Sink> Last<'_, S> {
    fn say(&mut self, w: &'static str) {
        if let Some(h) = self.held.take() {
            self.sink.word(h.as_bytes());
        }
        self.held = Some(w);
    }

    fn finish(self, form: Form) {
        let Some(h) = self.held else { return };
        let ord = || ORDINALS.iter().find(|(c, _)| *c == h).map_or(h, |(_, o)| *o);
        match form {
            Form::Cardinal => self.sink.word(h.as_bytes()),
            Form::Ordinal => self.sink.word(ord().as_bytes()),
            Form::Plural => {
                self.sink.word(DECADES.iter().find(|(c, _)| *c == h).map_or(h, |(_, p)| *p).as_bytes());
            }
            Form::OrdinalPlural => {
                let o = ord().as_bytes();
                let mut b = [0u8; 16];
                let n = o.len().min(15);
                b[..n].copy_from_slice(&o[..n]);
                b[n] = b's';
                self.sink.word(&b[..=n]);
            }
        }
    }
}

fn below_thousand<S: Sink>(n: u64, l: &mut Last<S>) {
    let h = n / 100;
    let r = n % 100;
    if h > 0 {
        l.say(ONES[h as usize]);
        l.say("hundred");
    }
    if r >= 20 {
        l.say(TENS[(r / 10) as usize]);
        if r % 10 != 0 {
            l.say(ONES[(r % 10) as usize]);
        }
    } else if r > 0 || h == 0 {
        l.say(ONES[r as usize]);
    }
}

fn cardinal_into<S: Sink>(n: u64, l: &mut Last<S>) {
    if n == 0 {
        l.say("zero");
        return;
    }
    let mut groups = [0u64; 5];
    let mut m = n.min(MAX_CARDINAL);
    for g in groups.iter_mut() {
        *g = m % 1000;
        m /= 1000;
    }
    for k in (0..5).rev() {
        if groups[k] != 0 {
            below_thousand(groups[k], l);
            if k > 0 {
                l.say(SCALES[k]);
            }
        }
    }
}

/// Says `n` (at most [`MAX_CARDINAL`]) in the given form.
pub fn number<S: Sink>(n: u64, form: Form, sink: &mut S) {
    let mut l = Last { sink, held: None };
    cardinal_into(n, &mut l);
    l.finish(form);
}

/// Says `n` as a cardinal.
pub fn cardinal<S: Sink>(n: u64, sink: &mut S) {
    number(n, Form::Cardinal, sink);
}

/// Says `n` as an ordinal.
pub fn ordinal<S: Sink>(n: u64, sink: &mut S) {
    number(n, Form::Ordinal, sink);
}

/// Whether `n` is read as a year when it stands alone (1100–2099).
pub fn is_year(n: u64) -> bool {
    (1100..=2099).contains(&n)
}

/// Says a year: "nineteen eighty-four", "nineteen hundred", "two thousand
/// five", "twenty twenty-six"; `form` applies to the last word ("the
/// nineteen nineties").
pub fn year<S: Sink>(n: u64, form: Form, sink: &mut S) {
    if !(1000..=9999).contains(&n) || (2000..2010).contains(&n) || n % 1000 == 0 {
        number(n, form, sink);
        return;
    }
    let hi = n / 100;
    let lo = n % 100;
    let mut l = Last { sink, held: None };
    cardinal_into(hi, &mut l);
    if lo == 0 {
        l.say("hundred");
    } else if lo < 10 {
        l.say("oh");
        l.say(ONES[lo as usize]);
    } else {
        cardinal_into(lo, &mut l);
    }
    l.finish(form);
}

/// Says each ASCII digit of `digits` ("zero", "one", …); other bytes are
/// skipped.
pub fn digits<S: Sink>(digits: &[u8], sink: &mut S) {
    for &d in digits {
        if d.is_ascii_digit() {
            sink.word(ONES[usize::from(d - b'0')].as_bytes());
        }
    }
}

/// Parses ASCII digits (at most 18); `None` if empty, too long or not all
/// digits.
pub fn parse(d: &[u8]) -> Option<u64> {
    if d.is_empty() || d.len() > 18 || !d.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(d.iter().fold(0u64, |a, &c| a * 10 + u64::from(c - b'0')))
}

/// Says an integer written in `d` (digits only): as a cardinal when it fits
/// and has no leading zero (a two-digit "01" counts as one), else digit by
/// digit.
pub fn integer<S: Sink>(d: &[u8], form: Form, sink: &mut S) {
    let lead_zero = d.len() > 1 && d[0] == b'0';
    match parse(d) {
        Some(n) if n <= MAX_CARDINAL && (!lead_zero || d.len() == 2) => number(n, form, sink),
        _ => digits(d, sink),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::normalise::testing::Words;

    fn card(n: u64) -> std::string::String {
        let mut w = Words::default();
        cardinal(n, &mut w);
        w.text()
    }

    #[test]
    fn cardinals() {
        assert_eq!(card(0), "zero");
        assert_eq!(card(7), "seven");
        assert_eq!(card(13), "thirteen");
        assert_eq!(card(20), "twenty");
        assert_eq!(card(42), "forty two");
        assert_eq!(card(100), "one hundred");
        assert_eq!(card(101), "one hundred one");
        assert_eq!(card(1000), "one thousand");
        assert_eq!(card(1_000_001), "one million one");
        assert_eq!(
            card(123_456_789),
            "one hundred twenty three million four hundred fifty six thousand seven hundred eighty nine"
        );
        assert_eq!(card(MAX_CARDINAL).split(' ').next(), Some("nine"));
        assert!(card(MAX_CARDINAL).contains("trillion"));
    }

    #[test]
    fn ordinals_years_digits() {
        let mut w = Words::default();
        ordinal(1, &mut w);
        ordinal(22, &mut w);
        ordinal(100, &mut w);
        ordinal(1012, &mut w);
        assert_eq!(w.text(), "first twenty second one hundredth one thousand twelfth");
        let mut w = Words::default();
        for y in [1984, 1900, 2000, 2005, 2010, 2026, 1066, 1905] {
            year(y, Form::Cardinal, &mut w);
        }
        assert_eq!(
            w.text(),
            "nineteen eighty four nineteen hundred two thousand two thousand five twenty ten twenty twenty six ten sixty six nineteen oh five"
        );
        let mut w = Words::default();
        year(1990, Form::Plural, &mut w);
        year(2000, Form::Plural, &mut w);
        assert_eq!(w.text(), "nineteen nineties two thousands");
        let mut w = Words::default();
        integer(b"007", Form::Cardinal, &mut w);
        integer(b"01", Form::Cardinal, &mut w);
        integer(b"1234567890123456", Form::Cardinal, &mut w);
        assert!(w.text().starts_with("zero zero seven one one two three"));
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"12a"), None);
        assert_eq!(parse(b"0042"), Some(42));
    }
}
