//! English text normalisation: one white-space-delimited chunk of text (with
//! up to two following chunks as look-ahead) into spoken words, spelled
//! letters and pauses.
//!
//! What it reads (see the tests for examples):
//!
//! * **numbers**: cardinals to 15 digits, ordinals (`21st`), decimals,
//!   signs, comma groups, versions (`1.2.3`), addresses (`192.168.0.1`),
//!   years (`1984`, `2026`), decades (`1990s`, `'80s`), fractions (`3/4`),
//!   ranges (`10-20`), phone and serial numbers digit by digit (`555-1234`,
//!   `1-800-555-0199`, `007`), digit strings over 15 digits;
//! * **times** (`12:30`, `7:05pm`, `7 pm`, `7:00`), **dates** (`2026-11-25`,
//!   `11/25/2026`, `25 Nov 2026`, `November 25th, 2026`), **currency**
//!   (`$3.50`, `€20`, `£1.5m`, `20€`, `99¢`), **percentages**, **units**
//!   (`44.1 kHz`, `320kbps`, `5 MB`, `10ms`, `-3 dB`, `20°C`),
//!   **abbreviations** (`Mr.`, `Dr.`, `St.`, `etc.`, `vs.`, `e.g.`, `No. 5`),
//!   **initials** (`J. R. R.`, `U.S.A.`), **Roman numerals** after title
//!   words and names (`Chapter IV`, `Henry VIII`);
//! * **file names**: a known extension is dropped, `_`, `-` and `.` part
//!   words, camelCase and digits split (`01 - Artist - Title.mp3`,
//!   `my_fileName2.flac`); **URLs and e-mail addresses** with "dot",
//!   "slash", "at";
//! * **punctuation** as pauses with intonation (comma, period, question —
//!   rising, or falling after a wh-word — exclamation, colon, semicolon,
//!   ellipsis, dashes, parentheses, quotes);
//! * **other scripts** are skipped; accented Latin is read without the
//!   accents; Greek letters and common symbols by name; strings that cannot
//!   be pronounced (no vowel, long consonant runs, short all-capitals
//!   acronyms) are spelled; [`Options::spell`] spells everything.

use crate::fold::{self, *};
use crate::number::{self, Form, MAX_CARDINAL};
use crate::prosody::{BreakKind, Contour};

/// Where the normaliser's output goes.
pub trait Sink {
    /// A word to pronounce (ASCII letters and `'`, at most [`MAX_WORD`]).
    fn word(&mut self, w: &[u8]);
    /// A letter to say by name.
    fn letter(&mut self, c: u8);
    /// A pause with its contour.
    fn brk(&mut self, kind: BreakKind, contour: Contour);
}

/// Longest word handed to the sink (longer letter runs are cut).
pub const MAX_WORD: usize = 32;
/// Longest chunk (bytes of UTF-8) normalised as one; the chunker cuts
/// longer runs without white space.
pub const MAX_CHUNK: usize = 48;

/// Normaliser options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Spell everything character by character (letters, digits,
    /// punctuation by name, "capital" before upper case).
    pub spell: bool,
    /// A single line end is a pause (menus, lists); otherwise only a blank
    /// line is.
    pub lines: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prev {
    Other,
    /// A word after which a Roman numeral is a cardinal ("chapter").
    Title,
    /// A capitalised word, after which a Roman numeral is regnal.
    Name,
}

#[derive(Clone, Copy, Debug)]
struct State {
    sentence_start: bool,
    wh: bool,
    last: Option<BreakKind>,
    prev: Prev,
    any: bool,
}

impl State {
    const fn new() -> State {
        State { sentence_start: true, wh: false, last: None, prev: Prev::Other, any: false }
    }
}

/// The normaliser's state between chunks.
#[derive(Clone, Copy, Debug)]
pub struct Normaliser {
    opts: Options,
    st: State,
}

const WH: [&str; 9] = ["what", "where", "when", "why", "who", "whom", "whose", "which", "how"];

/// Words after which a Roman numeral is read as a cardinal.
const ROMAN_CONTEXT: [&str; 21] = [
    "chapter", "book", "part", "volume", "vol", "act", "scene", "section", "appendix", "psalm", "canto",
    "phase", "level", "round", "stage", "step", "episode", "season", "war", "series", "mark",
];

/// Short words read as words, not spelled, in capitals.
const COMMON_SHORT: &[&str] = &[
    "a", "i", "am", "an", "as", "at", "be", "by", "do", "go", "he", "hi", "if", "in", "is", "it", "me", "my",
    "no", "of", "oh", "ok", "on", "or", "so", "to", "up", "us", "we", "ye", "all", "and", "any", "are",
    "ask", "bad", "bed", "big", "box", "boy", "but", "buy", "bye", "can", "car", "cat", "cry", "cut", "day",
    "did", "die", "dog", "eat", "end", "eye", "far", "few", "fly", "for", "fun", "get", "god", "got", "guy",
    "had", "has", "her", "hey", "him", "his", "hit", "hot", "how", "its", "joy", "key", "kid", "let", "lie",
    "lot", "low", "man", "may", "men", "mix", "new", "not", "now", "off", "old", "one", "our", "out", "own",
    "pop", "put", "red", "run", "sad", "say", "sea", "see", "she", "sky", "son", "sun", "the", "too", "top",
];

const COMMON_SHORT2: &[&str] =
    &["try", "two", "use", "war", "was", "way", "who", "why", "win", "won", "yes", "yet", "you", "wow"];

/// File extensions dropped from file names.
const EXTENSIONS: [&str; 44] = [
    "mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "aac", "wma", "aiff", "aif", "ape", "wv", "mpc",
    "spx", "mid", "midi", "kar", "txt", "md", "epub", "fb2", "pdf", "jpg", "jpeg", "png", "gif", "bmp",
    "webp", "cue", "m3u", "m3u8", "pls", "lrc", "log", "csv", "zip", "ofw", "bin", "doc", "docx", "htm",
    "html", "json",
];

/// Top-level domains spelled rather than read as a word.
const SPELL_TLD: [&str; 16] =
    ["io", "uk", "us", "de", "fr", "tv", "fm", "ai", "edu", "eu", "jp", "cn", "ru", "nl", "se", "au"];

/// Top-level domains that make a dotted name a host name.
const TLDS: [&str; 30] = [
    "com", "org", "net", "edu", "gov", "io", "dev", "app", "info", "biz", "co", "uk", "us", "de", "fr", "eu",
    "tv", "fm", "ai", "me", "jp", "cn", "ru", "nl", "se", "au", "ca", "ch", "es", "it",
];

/// Month names (lower case, as spoken).
pub const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

const MONTH_ABBR: [(&str, u8); 13] = [
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
    ("may", 5),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Abbr {
    /// Before a name; never ends a sentence ("Mr.").
    Title,
    /// May end a sentence when the next chunk is capitalised ("etc.").
    Plain,
    /// Only before a number ("No. 5").
    Num,
    /// Needs a capital and a period ("Wed.").
    Day,
}

/// Abbreviations (lower case without the period) and what is said.
const ABBREVIATIONS: [(&str, &str, Abbr); 60] = [
    ("mr", "mister", Abbr::Title),
    ("mrs", "missus", Abbr::Title),
    ("ms", "miz", Abbr::Title),
    ("dr", "doctor", Abbr::Title),
    ("prof", "professor", Abbr::Title),
    ("rev", "reverend", Abbr::Title),
    ("hon", "honorable", Abbr::Title),
    ("gen", "general", Abbr::Title),
    ("gov", "governor", Abbr::Title),
    ("sen", "senator", Abbr::Title),
    ("rep", "representative", Abbr::Title),
    ("capt", "captain", Abbr::Title),
    ("lt", "lieutenant", Abbr::Title),
    ("col", "colonel", Abbr::Title),
    ("sgt", "sergeant", Abbr::Title),
    ("maj", "major", Abbr::Title),
    ("fr", "father", Abbr::Title),
    ("mt", "mount", Abbr::Title),
    ("ft", "fort", Abbr::Title),
    ("messrs", "messieurs", Abbr::Title),
    ("st", "saint", Abbr::Title),
    ("jr", "junior", Abbr::Plain),
    ("sr", "senior", Abbr::Plain),
    ("etc", "et cetera", Abbr::Plain),
    ("vs", "versus", Abbr::Plain),
    ("inc", "incorporated", Abbr::Plain),
    ("ltd", "limited", Abbr::Plain),
    ("co", "company", Abbr::Plain),
    ("corp", "corporation", Abbr::Plain),
    ("dept", "department", Abbr::Plain),
    ("univ", "university", Abbr::Plain),
    ("approx", "approximately", Abbr::Plain),
    ("govt", "government", Abbr::Plain),
    ("misc", "miscellaneous", Abbr::Plain),
    ("feat", "featuring", Abbr::Plain),
    ("ave", "avenue", Abbr::Plain),
    ("blvd", "boulevard", Abbr::Plain),
    ("rd", "road", Abbr::Plain),
    ("hwy", "highway", Abbr::Plain),
    ("bros", "brothers", Abbr::Plain),
    ("max", "maximum", Abbr::Plain),
    ("avg", "average", Abbr::Plain),
    ("cf", "compare", Abbr::Plain),
    ("no", "number", Abbr::Num),
    ("nos", "numbers", Abbr::Num),
    ("p", "page", Abbr::Num),
    ("pp", "pages", Abbr::Num),
    ("vol", "volume", Abbr::Num),
    ("ch", "chapter", Abbr::Num),
    ("chap", "chapter", Abbr::Num),
    ("fig", "figure", Abbr::Num),
    ("op", "opus", Abbr::Num),
    ("sec", "section", Abbr::Num),
    ("mon", "monday", Abbr::Day),
    ("tue", "tuesday", Abbr::Day),
    ("tues", "tuesday", Abbr::Day),
    ("wed", "wednesday", Abbr::Day),
    ("thu", "thursday", Abbr::Day),
    ("fri", "friday", Abbr::Day),
    ("sat", "saturday", Abbr::Day),
];

/// A unit after a number: (written, singular, plural, may stand apart).
type Unit = (&'static [u8], &'static str, &'static str, bool);

const UNITS: [Unit; 51] = [
    (b"Hz", "hertz", "hertz", true),
    (b"kHz", "kilohertz", "kilohertz", true),
    (b"MHz", "megahertz", "megahertz", true),
    (b"GHz", "gigahertz", "gigahertz", true),
    (b"kB", "kilobyte", "kilobytes", true),
    (b"KB", "kilobyte", "kilobytes", true),
    (b"MB", "megabyte", "megabytes", true),
    (b"GB", "gigabyte", "gigabytes", true),
    (b"TB", "terabyte", "terabytes", true),
    (b"KiB", "kibibyte", "kibibytes", true),
    (b"MiB", "mebibyte", "mebibytes", true),
    (b"GiB", "gibibyte", "gibibytes", true),
    (b"kbps", "kilobit per second", "kilobits per second", true),
    (b"kb/s", "kilobit per second", "kilobits per second", true),
    (b"Mbps", "megabit per second", "megabits per second", true),
    (b"ms", "millisecond", "milliseconds", true),
    (&[MICRO, b's'], "microsecond", "microseconds", true),
    (b"ns", "nanosecond", "nanoseconds", true),
    (b"sec", "second", "seconds", true),
    (b"secs", "second", "seconds", true),
    (b"min", "minute", "minutes", true),
    (b"mins", "minute", "minutes", true),
    (b"hr", "hour", "hours", true),
    (b"hrs", "hour", "hours", true),
    (b"h", "hour", "hours", false),
    (b"mm", "millimeter", "millimeters", true),
    (b"cm", "centimeter", "centimeters", true),
    (b"m", "meter", "meters", false),
    (b"km", "kilometer", "kilometers", true),
    (b"km/h", "kilometer per hour", "kilometers per hour", true),
    (b"mph", "mile per hour", "miles per hour", true),
    (b"kg", "kilogram", "kilograms", true),
    (b"g", "gram", "grams", false),
    (b"mg", "milligram", "milligrams", true),
    (b"lb", "pound", "pounds", true),
    (b"lbs", "pound", "pounds", true),
    (b"oz", "ounce", "ounces", true),
    (b"ft", "foot", "feet", true),
    (b"mAh", "milliamp hour", "milliamp hours", true),
    (b"mA", "milliamp", "milliamps", true),
    (b"V", "volt", "volts", false),
    (b"mV", "millivolt", "millivolts", true),
    (b"W", "watt", "watts", false),
    (b"mW", "milliwatt", "milliwatts", true),
    (b"dB", "decibel", "decibels", true),
    (b"dBFS", "decibel full scale", "decibels full scale", true),
    (&[DEG, b'C'], "degree celsius", "degrees celsius", true),
    (&[DEG, b'F'], "degree fahrenheit", "degrees fahrenheit", true),
    (&[DEG], "degree", "degrees", true),
    (&[OHM], "ohm", "ohms", true),
    (&[GREEK + 23], "ohm", "ohms", true),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cur {
    Dollar,
    Euro,
    Pound,
    Yen,
    Cent,
}

impl Cur {
    fn of(b: u8) -> Option<Cur> {
        match b {
            b'$' => Some(Cur::Dollar),
            EURO => Some(Cur::Euro),
            POUND => Some(Cur::Pound),
            YEN => Some(Cur::Yen),
            CENT => Some(Cur::Cent),
            _ => None,
        }
    }

    /// (singular, plural, subunit singular, subunit plural)
    fn words(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Cur::Dollar => ("dollar", "dollars", "cent", "cents"),
            Cur::Euro => ("euro", "euros", "cent", "cents"),
            Cur::Pound => ("pound", "pounds", "penny", "pence"),
            Cur::Yen => ("yen", "yen", "", ""),
            Cur::Cent => ("cent", "cents", "", ""),
        }
    }
}

/// Words the normaliser says on its own (numbers, months, units, symbol
/// names, abbreviations' expansions…). The lexicon tool forces them into
/// the lexicon when the rules get them wrong; a test checks every table
/// above is covered.
pub const VOCABULARY: &[&str] = &[
    // numbers
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
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "trillion",
    "zeroth",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
    "eleventh",
    "twelfth",
    "thirteenth",
    "fourteenth",
    "fifteenth",
    "sixteenth",
    "seventeenth",
    "eighteenth",
    "nineteenth",
    "twentieth",
    "thirtieth",
    "fortieth",
    "fiftieth",
    "sixtieth",
    "seventieth",
    "eightieth",
    "ninetieth",
    "hundredth",
    "thousandth",
    "millionth",
    "billionth",
    "trillionth",
    "tens",
    "twenties",
    "thirties",
    "forties",
    "fifties",
    "sixties",
    "seventies",
    "eighties",
    "nineties",
    "thousands",
    "oh",
    "point",
    "minus",
    "plus",
    "about",
    "number",
    "numbers",
    "percent",
    "half",
    "halves",
    "quarter",
    "quarters",
    "to",
    "by",
    "times",
    "dot",
    "and",
    "of",
    "the",
    "o'clock",
    "seconds",
    "or",
    "over",
    // months, days
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    // currency
    "dollar",
    "dollars",
    "euro",
    "euros",
    "cent",
    "cents",
    "pound",
    "pounds",
    "penny",
    "pence",
    "yen",
    // units
    "hertz",
    "kilohertz",
    "megahertz",
    "gigahertz",
    "kilobyte",
    "kilobytes",
    "megabyte",
    "megabytes",
    "gigabyte",
    "gigabytes",
    "terabyte",
    "terabytes",
    "kibibyte",
    "kibibytes",
    "mebibyte",
    "mebibytes",
    "gibibyte",
    "gibibytes",
    "kilobit",
    "kilobits",
    "megabit",
    "megabits",
    "per",
    "millisecond",
    "milliseconds",
    "microsecond",
    "microseconds",
    "nanosecond",
    "nanoseconds",
    "minute",
    "minutes",
    "hour",
    "hours",
    "millimeter",
    "millimeters",
    "centimeter",
    "centimeters",
    "meter",
    "meters",
    "kilometer",
    "kilometers",
    "mile",
    "miles",
    "kilogram",
    "kilograms",
    "gram",
    "grams",
    "milligram",
    "milligrams",
    "ounce",
    "ounces",
    "foot",
    "feet",
    "milliamp",
    "milliamps",
    "volt",
    "volts",
    "millivolt",
    "millivolts",
    "watt",
    "watts",
    "milliwatt",
    "milliwatts",
    "decibel",
    "decibels",
    "full",
    "scale",
    "degree",
    "degrees",
    "celsius",
    "fahrenheit",
    "ohm",
    "ohms",
    // abbreviations
    "mister",
    "missus",
    "miz",
    "doctor",
    "professor",
    "reverend",
    "honorable",
    "general",
    "governor",
    "senator",
    "representative",
    "captain",
    "lieutenant",
    "colonel",
    "sergeant",
    "major",
    "father",
    "mount",
    "fort",
    "messieurs",
    "saint",
    "street",
    "junior",
    "senior",
    "et",
    "cetera",
    "versus",
    "incorporated",
    "limited",
    "company",
    "corporation",
    "department",
    "university",
    "approximately",
    "government",
    "miscellaneous",
    "featuring",
    "avenue",
    "boulevard",
    "road",
    "highway",
    "brothers",
    "maximum",
    "average",
    "compare",
    "page",
    "pages",
    "volume",
    "chapter",
    "figure",
    "opus",
    "section",
    "for",
    "example",
    "that",
    "is",
    // symbols, separators
    "slash",
    "backslash",
    "dash",
    "underscore",
    "at",
    "colon",
    "semicolon",
    "question",
    "mark",
    "exclamation",
    "equals",
    "hash",
    "tilde",
    "sharp",
    "copyright",
    "registered",
    "trademark",
    "paragraph",
    "divided",
    "micro",
    "less",
    "than",
    "greater",
    "comma",
    "period",
    "apostrophe",
    "quote",
    "star",
    "caret",
    "dollar",
    "left",
    "right",
    "paren",
    "bracket",
    "brace",
    "backtick",
    "bar",
    "capital",
    "ellipsis",
    "bullet",
    // Greek letters
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "zeta",
    "eta",
    "theta",
    "iota",
    "kappa",
    "lambda",
    "mu",
    "nu",
    "xi",
    "omicron",
    "pi",
    "rho",
    "sigma",
    "tau",
    "upsilon",
    "phi",
    "chi",
    "psi",
    "omega",
    // URLs
    "com",
    "org",
    "net",
    "gov",
    "dev",
    "app",
    "info",
    "biz",
    "ok",
];

fn lower_eq(a: &[u8], b: &str) -> bool {
    a.len() == b.len() && a.iter().zip(b.bytes()).all(|(x, y)| x.to_ascii_lowercase() == y)
}

fn is_letter(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_upper(b: u8) -> bool {
    b.is_ascii_uppercase()
}

fn is_vowel(b: u8) -> bool {
    matches!(b.to_ascii_lowercase(), b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

const ELISIONS: [&str; 9] = ["em", "cause", "til", "tis", "twas", "n", "bout", "round", "cos"];

fn is_lead(c: &[u8], i: usize) -> bool {
    match c[i] {
        b'(' | b'[' | b'{' | b'"' | LQUOTE | RQUOTE | INVQ | INVEXCL | b'*' | b'_' | b'<' => true,
        b'\'' => {
            let rest = &c[i + 1..];
            let run = rest.iter().take_while(|b| is_letter(**b)).count();
            !(rest.first().is_some_and(|b| is_digit(*b))
                || ELISIONS.iter().any(|e| lower_eq(&rest[..run], e)))
        }
        _ => false,
    }
}

fn is_trail(b: u8) -> bool {
    matches!(
        b,
        b',' | b';'
            | b':'
            | b'.'
            | b'!'
            | b'?'
            | b')'
            | b']'
            | b'}'
            | b'"'
            | b'\''
            | b'>'
            | LQUOTE
            | RQUOTE
            | ELLIPSIS
            | b'*'
            | b'_'
    )
}

/// Splits a folded chunk into leading punctuation, core and trailing
/// punctuation.
fn split(c: &[u8]) -> (&[u8], &[u8], &[u8]) {
    let mut s = 0;
    while s < c.len() && is_lead(c, s) {
        s += 1;
    }
    let mut e = c.len();
    while e > s && is_trail(c[e - 1]) {
        e -= 1;
    }
    // "dogs'" keeps no apostrophe; a number's "%" is part of the core.
    (&c[..s], &c[s..e], &c[e..])
}

/// Parses a canonical Roman numeral (upper case) up to 3999.
fn roman(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 15 {
        return None;
    }
    let val = |c: u8| match c {
        b'I' => Some(1),
        b'V' => Some(5),
        b'X' => Some(10),
        b'L' => Some(50),
        b'C' => Some(100),
        b'D' => Some(500),
        b'M' => Some(1000),
        _ => None,
    };
    let mut total = 0i32;
    for i in 0..s.len() {
        let v = val(s[i])?;
        let next = s.get(i + 1).and_then(|&c| val(c)).unwrap_or(0);
        if v < next { total -= v } else { total += v }
    }
    let total = u32::try_from(total).ok()?;
    // Canonical form check: re-encode and compare.
    let mut buf = [0u8; 16];
    let mut n = 0;
    let mut x = total;
    for (v, sym) in [
        (1000, &b"M"[..]),
        (900, b"CM"),
        (500, b"D"),
        (400, b"CD"),
        (100, b"C"),
        (90, b"XC"),
        (50, b"L"),
        (40, b"XL"),
        (10, b"X"),
        (9, b"IX"),
        (5, b"V"),
        (4, b"IV"),
        (1, b"I"),
    ] {
        while x >= v {
            for &c in sym {
                if n < buf.len() {
                    buf[n] = c;
                    n += 1;
                }
            }
            x -= v;
        }
    }
    (total > 0 && total < 4000 && &buf[..n] == s).then_some(total)
}

fn pronounceable(p: &[u8]) -> bool {
    if !p.iter().any(|&b| is_vowel(b)) {
        return false;
    }
    let mut run = 0;
    let mut initial = true;
    for &b in p {
        if b == b'\'' {
            continue;
        }
        if is_vowel(b) {
            run = 0;
            initial = false;
        } else {
            run += 1;
            if (initial && run > 3) || run > 5 {
                return false;
            }
        }
    }
    true
}

/// A Sink adapter that tracks sentences for the normaliser.
struct Cx<'a, S: Sink> {
    sink: &'a mut S,
    st: &'a mut State,
    opts: Options,
}

impl<S: Sink> Sink for Cx<'_, S> {
    fn word(&mut self, w: &[u8]) {
        if w.is_empty() {
            return;
        }
        if self.st.sentence_start {
            self.st.wh = WH.iter().any(|x| lower_eq(w, x));
            self.st.sentence_start = false;
        }
        self.st.last = None;
        self.st.any = true;
        for piece in w.chunks(MAX_WORD) {
            self.sink.word(piece);
        }
    }

    fn letter(&mut self, c: u8) {
        if self.st.sentence_start {
            self.st.wh = false;
            self.st.sentence_start = false;
        }
        self.st.last = None;
        self.st.any = true;
        self.sink.letter(c);
    }

    fn brk(&mut self, kind: BreakKind, contour: Contour) {
        if kind.ends_sentence() {
            self.st.sentence_start = true;
            self.st.prev = Prev::Other;
        }
        self.st.last = Some(kind);
        self.sink.brk(kind, contour);
    }
}

/// Result of a core handler: chunks used and bytes of the last chunk's
/// trailing punctuation already handled.
struct Res {
    chunks: usize,
    skip: usize,
}

const ONE: Res = Res { chunks: 1, skip: 0 };

impl<S: Sink> Cx<'_, S> {
    fn say(&mut self, words: &str) {
        for w in words.split(' ') {
            self.word(w.as_bytes());
        }
    }

    fn brk1(&mut self, kind: BreakKind) {
        self.brk(kind, kind.contour());
    }

    fn spell_letters(&mut self, s: &[u8]) {
        for &b in s {
            if is_letter(b) {
                self.letter(b);
            } else if is_digit(b) {
                self.word(number::ONES[usize::from(b - b'0')].as_bytes());
            }
        }
    }

    /// Spell mode: every character.
    fn spell(&mut self, c: &[u8]) {
        for &b in c {
            if is_letter(b) {
                if is_upper(b) {
                    self.say("capital");
                }
                self.letter(b);
            } else if is_digit(b) {
                self.word(number::ONES[usize::from(b - b'0')].as_bytes());
            } else if let Some(name) = spell_name(b) {
                self.say(name);
            }
        }
        self.brk1(BreakKind::Group);
    }

    fn lead(&mut self, b: u8) {
        match b {
            b'(' | b'[' | b'{' | b'<' => self.brk1(BreakKind::ParenOpen),
            b'"' | b'\'' | LQUOTE | RQUOTE => self.brk1(BreakKind::Quote),
            _ => {}
        }
    }

    fn trail(&mut self, t: &[u8]) {
        let mut i = 0;
        while i < t.len() {
            match t[i] {
                b'.' => {
                    let k = i + t[i..].iter().take_while(|&&b| b == b'.').count();
                    self.brk1(if k - i >= 2 { BreakKind::Ellipsis } else { BreakKind::Period });
                    i = k;
                    continue;
                }
                b'?' | b'!' => {
                    let k = i + t[i..].iter().take_while(|&&b| b == b'?' || b == b'!').count();
                    if t[i..k].contains(&b'?') {
                        let c = if self.st.wh { Contour::Fall } else { Contour::Rise };
                        self.brk(BreakKind::Question, c);
                    } else {
                        self.brk1(BreakKind::Exclamation);
                    }
                    i = k;
                    continue;
                }
                ELLIPSIS => self.brk1(BreakKind::Ellipsis),
                b',' => self.brk1(BreakKind::Comma),
                b';' => self.brk1(BreakKind::Semicolon),
                b':' => self.brk1(BreakKind::Colon),
                b')' | b']' | b'}' | b'>' => self.brk1(BreakKind::ParenClose),
                b'"' | LQUOTE | RQUOTE => self.brk1(BreakKind::Quote),
                _ => {}
            }
            i += 1;
        }
    }

    // ----------------------------------------------------------- dispatch

    fn core(&mut self, core: &[u8], trail: &[u8], la: [Option<&[u8]>; 2]) -> Res {
        if core.is_empty() {
            return ONE;
        }
        if !core.iter().any(|&b| is_alnum(b)) {
            self.symbols(core);
            return ONE;
        }
        if is_url(core) || is_email(core) {
            self.url(core);
            self.st.prev = Prev::Other;
            return ONE;
        }
        if let Some(r) = self.month_date(core, trail, la) {
            return r;
        }
        if let Some(r) = self.abbreviation(core, trail, la) {
            return r;
        }
        if let Some(r) = self.initialism(core, trail, la) {
            return r;
        }
        let first = core[0];
        let numeric = is_digit(first)
            || (matches!(first, b'.' | b'\'') && core.get(1).is_some_and(|b| is_digit(*b)))
            || (num_prefix(first) && core[1..].iter().take(3).any(|b| is_digit(*b)) && {
                let k = core.iter().take_while(|b| num_prefix(**b)).count();
                core.get(k).is_some_and(|b| is_digit(*b) || *b == b'.')
            });
        if numeric {
            let r = self.number(core, trail, la);
            self.st.prev = Prev::Other;
            return r;
        }
        if let Some(r) = self.roman_numeral(core) {
            return r;
        }
        self.mixed(core);
        ONE
    }

    // ----------------------------------------------------------- symbols

    fn symbols(&mut self, core: &[u8]) {
        if core.iter().all(|&b| matches!(b, b'-' | ENDASH | EMDASH)) {
            self.brk1(BreakKind::Dash);
            return;
        }
        for &b in core {
            self.symbol(b, true);
        }
    }

    /// One symbol: named, a pause, or nothing. `alone`: not inside a word.
    fn symbol(&mut self, b: u8, alone: bool) {
        match b {
            b'&' => self.say("and"),
            b'+' => self.say("plus"),
            b'=' => self.say("equals"),
            b'@' => self.say("at"),
            b'%' => self.say("percent"),
            b'<' if alone => self.say("less than"),
            b'>' if alone => self.say("greater than"),
            b'\\' => self.say("backslash"),
            b'|' | BULLET => self.brk1(BreakKind::Group),
            b'$' if alone => self.say("dollars"),
            ELLIPSIS => self.brk1(BreakKind::Ellipsis),
            ENDASH | EMDASH => self.brk1(BreakKind::Dash),
            LQUOTE | RQUOTE | b'"' => self.brk1(BreakKind::Quote),
            _ => {
                if let Some(n) = symbol_name(b) {
                    self.say(n);
                }
            }
        }
    }

    // ----------------------------------------------------------- URLs

    fn url(&mut self, s: &[u8]) {
        let mut s = s;
        if let Some(p) = find(s, b"://") {
            let scheme = &s[..p];
            if !(lower_eq(scheme, "http") || lower_eq(scheme, "https")) {
                self.spell_letters(scheme);
                self.say("colon slash slash");
            }
            s = &s[p + 3..];
        }
        if s.len() > 4 && lower_eq(&s[..4], "www.") {
            self.spell_letters(b"www");
            self.say("dot");
            s = &s[4..];
        }
        // The host's last label (for spelled top-level domains).
        let host_end = s.iter().position(|&b| b == b'/' || b == b'?' || b == b'#').unwrap_or(s.len());
        let tld_start = s[..host_end].iter().rposition(|&b| b == b'.').map_or(usize::MAX, |p| p + 1);
        let mut i = 0;
        while i < s.len() {
            let b = s[i];
            if is_letter(b) {
                let j = i + s[i..].iter().take_while(|b| is_letter(**b)).count();
                let run = &s[i..j];
                if run.len() == 1
                    || (i == tld_start && j == host_end && SPELL_TLD.iter().any(|t| lower_eq(run, t)))
                {
                    self.spell_letters(run);
                } else {
                    self.word_piece(run);
                }
                i = j;
            } else if is_digit(b) {
                let j = i + s[i..].iter().take_while(|b| is_digit(**b)).count();
                if j - i <= 2 {
                    number::integer(&s[i..j], Form::Cardinal, self);
                } else {
                    number::digits(&s[i..j], self);
                }
                i = j;
            } else {
                let last = i + 1 == s.len();
                match b {
                    b'.' => self.say("dot"),
                    b'/' if !last => self.say("slash"),
                    b'-' => self.say("dash"),
                    b'_' => self.say("underscore"),
                    b'@' => self.say("at"),
                    b':' => self.say("colon"),
                    b'?' => self.say("question mark"),
                    b'=' => self.say("equals"),
                    b'&' => self.say("and"),
                    b'#' => self.say("hash"),
                    b'~' => self.say("tilde"),
                    b'%' => self.say("percent"),
                    b'+' => self.say("plus"),
                    _ => {}
                }
                i += 1;
            }
        }
    }

    // ----------------------------------------------------------- words

    /// A run of letters (and inner apostrophes): camelCase pieces, each a
    /// word or spelled.
    fn letters(&mut self, run: &[u8]) {
        let mut start = 0;
        for k in 1..run.len() {
            let (a, b) = (run[k - 1], run[k]);
            let lower_upper = a.is_ascii_lowercase() && is_upper(b);
            let acronym_end = is_upper(a)
                && is_upper(b)
                && run.get(k + 1).is_some_and(|c| c.is_ascii_lowercase())
                && k - start >= 2;
            let mc = &run[start..k] == b"Mc" || &run[start..k] == b"Mac";
            if (lower_upper || acronym_end) && !mc {
                self.word_piece(&run[start..k]);
                start = k;
            }
        }
        self.word_piece(&run[start..]);
    }

    fn word_piece(&mut self, p: &[u8]) {
        if p.is_empty() {
            return;
        }
        let letters = p.iter().filter(|b| is_letter(**b)).count();
        let caps = p.iter().filter(|b| is_upper(**b)).count();
        let title_case = is_upper(p[0]) && caps == 1 && letters >= 2;
        let spell = if letters == 1 {
            match p[0] {
                b'a' | b'i' | b'I' => false,
                // "Plan A", "Type A": a letter; otherwise the article.
                b'A' => self.st.prev != Prev::Other,
                _ => true,
            }
        } else if caps == letters {
            if letters <= 3 {
                !COMMON_SHORT.iter().chain(COMMON_SHORT2.iter()).any(|w| lower_eq(p, w))
            } else {
                !pronounceable(p)
            }
        } else {
            !pronounceable(p)
        };
        if spell {
            self.spell_letters(p);
        } else {
            self.word(p);
        }
        self.st.prev = if ROMAN_CONTEXT.iter().any(|w| lower_eq(p, w)) {
            Prev::Title
        } else if title_case {
            Prev::Name
        } else {
            Prev::Other
        };
    }

    /// A chunk mixing letters, digits and symbols (file names, camelCase,
    /// `MP3`, `x86`, `well-known`, `C#`).
    fn mixed(&mut self, core: &[u8]) {
        let mut core = core;
        if let Some(dot) = core.iter().rposition(|&b| b == b'.') {
            let ext = &core[dot + 1..];
            if dot > 0 && EXTENSIONS.iter().any(|e| lower_eq(ext, e)) {
                core = &core[..dot];
            }
        }
        let mut i = 0;
        while i < core.len() {
            let b = core[i];
            if is_letter(b) {
                let mut j = i;
                while j < core.len()
                    && (is_letter(core[j])
                        || (core[j] == b'\'' && core.get(j + 1).is_some_and(|c| is_letter(*c)) && j > i))
                {
                    j += 1;
                }
                self.letters(&core[i..j]);
                // "C#", "F#": sharp.
                if j - i == 1 && core.get(j) == Some(&b'#') {
                    self.say("sharp");
                    j += 1;
                }
                i = j;
            } else if is_digit(b) {
                let mut j = i;
                while j < core.len()
                    && (is_digit(core[j])
                        || (core[j] == b'.' && core.get(j + 1).is_some_and(|c| is_digit(*c))))
                {
                    j += 1;
                }
                self.plain(&core[i..j], Form::Cardinal, true);
                i = j;
            } else {
                match b {
                    b'.' if i > 0
                        && is_letter(core[i - 1])
                        && core.get(i + 1).is_some_and(|c| is_letter(*c)) =>
                    {
                        self.say("dot");
                    }
                    b'_' | b'-' | b'/' | b'.' | b'\'' | b'*' | b'~' | b'^' | b'`' | b'"' | b'#' => {}
                    _ => self.symbol(b, false),
                }
                i += 1;
            }
        }
    }

    // ----------------------------------------------------------- abbreviations

    fn abbreviation(&mut self, core: &[u8], trail: &[u8], la: [Option<&[u8]>; 2]) -> Option<Res> {
        if core.len() > 7 || !core.iter().all(|b| is_letter(*b)) {
            // Dotted ones: e.g., i.e., a.m., p.m.
            let dotted: [(&[u8], &str, bool); 4] = [
                (b"e.g", "for example", false),
                (b"i.e", "that is", false),
                (b"a.m", "", true),
                (b"p.m", "", true),
            ];
            for (d, say, letters) in dotted {
                if core.len() == d.len() && core.iter().zip(d).all(|(a, b)| a.to_ascii_lowercase() == *b) {
                    if letters {
                        self.spell_letters(core);
                    } else {
                        self.say(say);
                    }
                    let skip = usize::from(trail.first() == Some(&b'.'));
                    return Some(Res { chunks: 1, skip });
                }
            }
            return None;
        }
        let period = trail.first() == Some(&b'.');
        let next = la[0].map(|n| split(n).1);
        let next_cap = next.is_some_and(|n| n.first().is_some_and(|b| is_upper(*b)));
        let next_num = next.is_some_and(|n| {
            n.first().is_some_and(|b| is_digit(*b))
                || (n.iter().all(|b| b"IVXLCDM".contains(b)) && !n.is_empty())
        });
        let capital = is_upper(core[0]);
        let (_, say, kind) = ABBREVIATIONS.iter().find(|(a, _, _)| lower_eq(core, a))?;
        let ok = match kind {
            // "Dr Smith" without the period too; "St." before a name is
            // Saint, elsewhere Street.
            Abbr::Title => capital && (period || next_cap) && (next_cap || lower_eq(core, "st")),
            Abbr::Plain => period || lower_eq(core, "vs") || lower_eq(core, "etc"),
            Abbr::Num => next_num && (period || lower_eq(core, "no") && capital),
            Abbr::Day => capital && period,
        };
        if !ok {
            return None;
        }
        if lower_eq(core, "st") && !next_cap {
            self.say("street");
        } else {
            self.say(say);
        }
        self.st.prev = if *kind == Abbr::Num { Prev::Title } else { Prev::Other };
        let skip = usize::from(period);
        // A plain abbreviation at a sentence end: its period counts twice.
        if period && *kind == Abbr::Plain && (next_cap || la[0].is_none()) {
            self.brk1(BreakKind::Period);
        }
        Some(Res { chunks: 1, skip })
    }

    /// `J.`, `U.S.A.`, `Ph.D.`: letters by name.
    fn initialism(&mut self, core: &[u8], trail: &[u8], la: [Option<&[u8]>; 2]) -> Option<Res> {
        let period = trail.first() == Some(&b'.');
        let single = core.len() == 1 && is_upper(core[0]) && period;
        let dotted = core.len() >= 3
            && core.contains(&b'.')
            && core
                .split(|&b| b == b'.')
                .all(|p| !p.is_empty() && p.len() <= 2 && p.iter().all(|b| is_letter(*b)))
            && core.iter().filter(|&&b| b == b'.').count() >= 1
            && (period || core.split(|&b| b == b'.').all(|p| p.len() == 1));
        if !single && !dotted {
            return None;
        }
        if single {
            // "J. Smith": an initial; "plan B." ends the sentence.
            let next_cap = la[0].is_some_and(|n| split(n).1.first().is_some_and(|b| is_upper(*b)));
            if !next_cap {
                return None;
            }
        }
        self.spell_letters(core);
        self.st.prev = Prev::Other;
        Some(Res { chunks: 1, skip: usize::from(period && la[0].is_some()) })
    }

    // ----------------------------------------------------------- dates

    fn month_of(word: &[u8]) -> Option<u8> {
        if word.len() < 3 || !is_upper(word[0]) {
            return None;
        }
        if let Some(i) = MONTHS.iter().position(|m| lower_eq(word, m)) {
            return Some(i as u8 + 1);
        }
        MONTH_ABBR.iter().find(|(a, _)| lower_eq(word, a)).map(|(_, m)| *m)
    }

    /// "November 25", "Nov. 25th, 2026".
    fn month_date(&mut self, core: &[u8], trail: &[u8], la: [Option<&[u8]>; 2]) -> Option<Res> {
        let month = Self::month_of(core)?;
        if !(trail.is_empty() || trail == b"." || trail == b",") {
            return None;
        }
        let (_, dcore, dtrail) = split(la[0]?);
        let day = day_number(dcore)?;
        self.word(MONTHS[usize::from(month - 1)].as_bytes());
        number::ordinal(u64::from(day), self);
        let mut chunks = 2;
        if (dtrail.is_empty() || dtrail == b",")
            && let Some(y) = la[1]
        {
            let (_, ycore, ytrail) = split(y);
            if ycore.len() == 4
                && let Some(yv) = number::parse(ycore)
            {
                number::year(yv, Form::Cardinal, self);
                self.trail(ytrail);
                chunks = 3;
            }
        }
        if chunks == 2 {
            self.trail(dtrail);
        }
        self.st.prev = Prev::Other;
        // The trails of the consumed chunks are emitted here; nothing of
        // this chunk's own (a period after "Nov") remains.
        Some(Res { chunks, skip: usize::MAX })
    }

    fn say_date(&mut self, month: u64, day: Option<u64>, year: Option<u64>, day_first: bool) {
        let m = MONTHS[(month.clamp(1, 12) - 1) as usize];
        match (day, day_first) {
            (Some(d), true) => {
                self.say("the");
                number::ordinal(d, self);
                self.say("of");
                self.say(m);
            }
            (Some(d), false) => {
                self.say(m);
                number::ordinal(d, self);
            }
            (None, _) => self.say(m),
        }
        if let Some(y) = year {
            number::year(y, Form::Cardinal, self);
        }
    }

    // ----------------------------------------------------------- Roman numerals

    fn roman_numeral(&mut self, core: &[u8]) -> Option<Res> {
        let v = roman(core)?;
        match self.st.prev {
            Prev::Title => number::cardinal(u64::from(v), self),
            Prev::Name if core != b"I" && core.iter().all(|b| b"IVXL".contains(b)) && v >= 2 => {
                self.say("the");
                number::ordinal(u64::from(v), self);
            }
            _ => return None,
        }
        self.st.prev = Prev::Other;
        Some(ONE)
    }

    // ----------------------------------------------------------- numbers

    /// A plain number: comma groups, decimals, versions, addresses.
    fn plain(&mut self, num: &[u8], form: Form, year_ok: bool) {
        let dots = num.iter().filter(|&&b| b == b'.').count();
        let commas = num.iter().filter(|&&b| b == b',').count();
        if commas > 0 {
            let groups_ok = {
                let mut it = num.split(|&b| b == b'.').next().unwrap_or(&[]).split(|&b| b == b',');
                let first = it.next().unwrap_or(&[]);
                (1..=3).contains(&first.len()) && it.all(|g| g.len() == 3)
            } && dots <= 1;
            if groups_ok {
                let mut buf = [0u8; 48];
                let mut n = 0;
                for &b in num {
                    if b != b',' && n < buf.len() {
                        buf[n] = b;
                        n += 1;
                    }
                }
                self.plain(&buf[..n], form, false);
            } else {
                for (k, part) in num.split(|&b| b == b',').enumerate() {
                    if k > 0 {
                        self.brk1(BreakKind::Comma);
                    }
                    self.plain(part, form, false);
                }
            }
            return;
        }
        match dots {
            0 => {
                if year_ok
                    && num.len() == 4
                    && num[0] != b'0'
                    && let Some(n) = number::parse(num)
                    && (number::is_year(n) || form == Form::Plural)
                {
                    number::year(n, form, self);
                    return;
                }
                number::integer(num, form, self);
            }
            1 => {
                let p = num.iter().position(|&b| b == b'.').unwrap_or(0);
                if p > 0 {
                    number::integer(&num[..p], Form::Cardinal, self);
                }
                self.say("point");
                number::digits(&num[p + 1..], self);
            }
            _ => {
                let sep = if dots >= 3 { "dot" } else { "point" };
                for (k, part) in num.split(|&b| b == b'.').enumerate() {
                    if k > 0 {
                        self.say(sep);
                    }
                    number::integer(part, Form::Cardinal, self);
                }
            }
        }
    }

    fn time(&mut self, h: &[u8], m: &[u8], s: Option<&[u8]>, ampm: Option<u8>) {
        number::integer(h, Form::Cardinal, self);
        let mv = number::parse(m).unwrap_or(0);
        if mv == 0 {
            if ampm.is_none() && s.is_none() {
                self.say("o'clock");
            }
        } else if mv < 10 {
            self.say("oh");
            number::cardinal(mv, self);
        } else {
            number::cardinal(mv, self);
        }
        if let Some(s) = s {
            let sv = number::parse(s).unwrap_or(0);
            if mv == 0 && sv > 0 {
                self.say("oh oh");
            }
            if sv > 0 {
                self.say("and");
                number::cardinal(sv, self);
                self.say(if sv == 1 { "second" } else { "seconds" });
            }
        }
        if let Some(c) = ampm {
            self.letter(c);
            self.letter(b'm');
        }
    }

    fn number(&mut self, core: &[u8], trail: &[u8], la: [Option<&[u8]>; 2]) -> Res {
        // Prefixes: sign, approximately, number sign, currency.
        let mut i = 0;
        let mut cur: Option<Cur> = None;
        let mut decade_mark = false;
        while i < core.len() && i < 3 {
            match core[i] {
                b'+' => self.say("plus"),
                b'-' | MINUS => self.say("minus"),
                b'~' => self.say("about"),
                PLUSMINUS => self.say("plus or minus"),
                b'#' => self.say("number"),
                b'\'' => decade_mark = true,
                b => match Cur::of(b) {
                    Some(c) => cur = Some(c),
                    None => break,
                },
            }
            i += 1;
        }
        let body = &core[i..];
        // The numeric part: digits with inner separators.
        let mut j = 0;
        while j < body.len() {
            let b = body[j];
            let next_digit = body.get(j + 1).is_some_and(|c| is_digit(*c));
            let inner = matches!(b, b',' | b'.' | b':' | b'/' | b'-' | b'x' | TIMES) && next_digit && j > 0;
            let leading_point = b == b'.' && j == 0 && next_digit;
            if is_digit(b) || inner || leading_point {
                j += 1;
            } else {
                break;
            }
        }
        let num = &body[..j];
        let mut suffix = &body[j..];
        let plain_number = num.iter().all(|&b| is_digit(b) || b == b'.' || b == b',');

        // Times and clock hours: "12:30", "7:05pm", "7pm", "7 pm".
        let ampm_of = |s: &[u8]| -> Option<u8> {
            let l: [u8; 4] = core::array::from_fn(|k| s.get(k).map_or(0, |c| c.to_ascii_lowercase()));
            match (s.len(), l) {
                (2, [a @ (b'a' | b'p'), b'm', _, _]) => Some(a),
                (3, [a @ (b'a' | b'p'), b'.', b'm', _]) => Some(a),
                _ => None,
            }
        };
        if num.contains(&b':') {
            let parts: [&[u8]; 3] = {
                let mut it = num.split(|&b| b == b':');
                [it.next().unwrap_or(&[]), it.next().unwrap_or(&[]), it.next().unwrap_or(&[])]
            };
            let colons = num.iter().filter(|&&b| b == b':').count();
            let valid = colons <= 2
                && (1..=2).contains(&parts[0].len())
                && parts[1].len() == 2
                && number::parse(parts[1]).is_some_and(|m| m < 60)
                && (colons == 1 || parts[2].len() == 2);
            if valid {
                let mut ampm = ampm_of(suffix);
                let mut res = ONE;
                if ampm.is_some() {
                    res.skip = usize::from(suffix.len() == 3 && trail.first() == Some(&b'.'));
                    suffix = &[];
                } else if suffix.is_empty()
                    && trail.is_empty()
                    && let Some(n) = la[0]
                {
                    let (_, c, _) = split(n);
                    let c2 = if c.len() == 3 && n.ends_with(b".") { &n[..4.min(n.len())] } else { c };
                    if let Some(a) = ampm_of(c2) {
                        ampm = Some(a);
                        res.chunks = 2;
                    }
                }
                self.time(parts[0], parts[1], (colons == 2).then_some(parts[2]), ampm);
                self.mixed(suffix);
                if res.chunks == 2 {
                    let (_, _, t) = split(la[0].unwrap_or(&[]));
                    let t = if t.first() == Some(&b'.') && t.len() >= 2 { &t[1..] } else { t };
                    self.trail(t);
                    return Res { chunks: 2, skip: usize::MAX };
                }
                return res;
            }
            for (k, p) in num.split(|&b| b == b':').enumerate() {
                if k > 0 {
                    // Bible verses, ratios: "3:16" → "three sixteen".
                }
                number::integer(p, Form::Cardinal, self);
            }
            self.mixed(suffix);
            return ONE;
        }
        if num.contains(&b'/') {
            let mut parts = [&b""[..]; 4];
            let mut n = 0;
            for p in num.split(|&b| b == b'/') {
                if n < 4 {
                    parts[n] = p;
                }
                n += 1;
            }
            if n == 3 {
                let (a, b, y) = (number::parse(parts[0]), number::parse(parts[1]), number::parse(parts[2]));
                if let (Some(a), Some(b), Some(y)) = (a, b, y) {
                    let (m, d, day_first) = if a > 12 { (b, a, true) } else { (a, b, false) };
                    if (1..=12).contains(&m) && (1..=31).contains(&d) {
                        let y = if parts[2].len() == 2 { 2000 + y } else { y };
                        self.say_date(m, Some(d), Some(y), day_first);
                        self.mixed(suffix);
                        return ONE;
                    }
                }
            }
            if n == 2
                && let (Some(a), Some(b)) = (number::parse(parts[0]), number::parse(parts[1]))
                && a < b
                && b <= 16
                && suffix.is_empty()
            {
                number::cardinal(a, self);
                match (b, a == 1) {
                    (2, true) => self.say("half"),
                    (2, false) => self.say("halves"),
                    (4, true) => self.say("quarter"),
                    (4, false) => self.say("quarters"),
                    (_, true) => number::ordinal(b, self),
                    (_, false) => number::number(b, Form::OrdinalPlural, self),
                }
                return ONE;
            }
            for p in num.split(|&b| b == b'/') {
                self.plain(p, Form::Cardinal, true);
            }
            self.mixed(suffix);
            return ONE;
        }
        if num.contains(&b'x') || num.contains(&TIMES) {
            for (k, p) in num.split(|&b| b == b'x' || b == TIMES).enumerate() {
                if k > 0 {
                    self.say("by");
                }
                self.plain(p, Form::Cardinal, false);
            }
            self.mixed(suffix);
            return ONE;
        }
        if num.contains(&b'-') {
            let mut groups = [&b""[..]; 8];
            let mut n = 0;
            for g in num.split(|&b| b == b'-') {
                if n < 8 {
                    groups[n] = g;
                }
                n += 1;
            }
            let n = n.min(8);
            let g = &groups[..n];
            // ISO date.
            if n == 3 && g[0].len() == 4 && g[1].len() == 2 && g[2].len() == 2 {
                let (y, m, d) = (number::parse(g[0]), number::parse(g[1]), number::parse(g[2]));
                if let (Some(y), Some(m), Some(d)) = (y, m, d)
                    && (1..=12).contains(&m)
                    && (1..=31).contains(&d)
                {
                    self.say_date(m, Some(d), Some(y), false);
                    self.mixed(suffix);
                    return ONE;
                }
            }
            let phone = n >= 3 || (n == 2 && g[0].len() == 3 && g[1].len() == 4);
            if phone || g.iter().any(|x| !x.iter().all(|b| is_digit(*b))) {
                for (k, x) in g.iter().enumerate() {
                    if k > 0 {
                        self.brk1(BreakKind::Group);
                    }
                    number::digits(x, self);
                }
            } else {
                // A range: "10-20", "1990-1995".
                self.plain(g[0], Form::Cardinal, true);
                self.say("to");
                self.plain(g[1], Form::Cardinal, true);
            }
            self.mixed(suffix);
            return ONE;
        }

        // A plain number with what follows it.
        let _ = plain_number;
        let int_value = if num.contains(&b'.') || num.contains(&b',') { None } else { number::parse(num) };
        let lower: [u8; 4] = core::array::from_fn(|k| suffix.get(k).map_or(0, |c| c.to_ascii_lowercase()));
        let is_ord = |s: &[u8]| {
            s.len() >= 2
                && matches!(
                    [s[0].to_ascii_lowercase(), s[1].to_ascii_lowercase()],
                    [b's', b't'] | [b'n', b'd'] | [b'r', b'd'] | [b't', b'h']
                )
                && !s.get(2).is_some_and(|c| is_letter(*c))
        };
        // Currency after the number ("20€").
        if cur.is_none()
            && let Some(&b) = suffix.first()
            && let Some(c) = Cur::of(b)
        {
            cur = Some(c);
            suffix = &suffix[1..];
        }
        if let Some(c) = cur {
            self.money(num, suffix, c);
            return ONE;
        }
        if int_value.is_some() && is_ord(suffix) {
            self.plain(num, Form::Ordinal, false);
            self.mixed(&suffix[2..]);
            return ONE;
        }
        if (lower[..2] == *b"s\0" || suffix == b"'s") && int_value.is_some() && num.ends_with(b"0") {
            self.plain(num, Form::Plural, true);
            return ONE;
        }
        if decade_mark && suffix == b"s" {
            self.plain(num, Form::Plural, false);
            return ONE;
        }
        if suffix == b"%" {
            self.plain(num, Form::Cardinal, false);
            self.say("percent");
            return ONE;
        }
        if suffix.len() == 1 && matches!(suffix[0], b'x' | b'X' | TIMES) {
            self.plain(num, Form::Cardinal, false);
            self.say("times");
            return ONE;
        }
        if let Some(a) = ampm_of(suffix)
            && int_value.is_some_and(|h| (1..=12).contains(&h))
        {
            number::integer(num, Form::Cardinal, self);
            self.letter(a);
            self.letter(b'm');
            return Res { chunks: 1, skip: usize::from(suffix.len() == 3 && trail.first() == Some(&b'.')) };
        }
        let one = int_value == Some(1);
        if !suffix.is_empty() {
            if let Some((_, s, p, _)) = UNITS.iter().find(|u| u.0 == suffix) {
                self.plain(num, Form::Cardinal, false);
                self.say(if one { s } else { p });
                return ONE;
            }
            // "21st-century", "4K", "3D", "100GB/s".
            self.plain(num, Form::Cardinal, false);
            self.mixed(suffix);
            return ONE;
        }
        // Look-ahead: a unit, "%", "am"/"pm", a month.
        if trail.is_empty()
            && let Some(n) = la[0]
        {
            let (_, c, t) = split(n);
            if let Some((_, s, p, true)) = UNITS.iter().find(|u| u.0 == c) {
                self.plain(num, Form::Cardinal, false);
                self.say(if one { s } else { p });
                self.trail(t);
                return Res { chunks: 2, skip: usize::MAX };
            }
            if c == b"%" {
                self.plain(num, Form::Cardinal, false);
                self.say("percent");
                self.trail(t);
                return Res { chunks: 2, skip: usize::MAX };
            }
            let dotted = c.len() == 3 && t.first() == Some(&b'.');
            if let Some(a) = ampm_of(c)
                && int_value.is_some_and(|h| (1..=12).contains(&h))
            {
                number::integer(num, Form::Cardinal, self);
                self.letter(a);
                self.letter(b'm');
                self.trail(if dotted { &t[1..] } else { t });
                return Res { chunks: 2, skip: usize::MAX };
            }
            if let Some(d) = int_value.filter(|d| (1..=31).contains(d))
                && let Some(m) = Self::month_of(c)
            {
                let mut year = None;
                let mut chunks = 2;
                let mut last_trail = t;
                if (t.is_empty() || t == b"," || t == b".")
                    && let Some(y) = la[1]
                {
                    let (_, yc, yt) = split(y);
                    if yc.len() == 4
                        && let Some(yv) = number::parse(yc)
                    {
                        year = Some(yv);
                        chunks = 3;
                        last_trail = yt;
                    }
                }
                self.say_date(u64::from(m), Some(d), year, true);
                let last_trail = if chunks == 2 && t == b"." && MONTH_ABBR.iter().any(|(a, _)| lower_eq(c, a))
                {
                    &t[1..]
                } else {
                    last_trail
                };
                self.trail(last_trail);
                return Res { chunks, skip: usize::MAX };
            }
        }
        let _ = lower;
        self.plain(num, Form::Cardinal, true);
        ONE
    }

    fn money(&mut self, num: &[u8], suffix: &[u8], c: Cur) {
        let (one, many, sub1, subn) = c.words();
        // A scale word: "$1.5M", "£2bn", "€3k".
        let scale = match suffix {
            b"k" | b"K" => Some("thousand"),
            b"m" | b"M" | b"mn" => Some("million"),
            b"b" | b"B" | b"bn" => Some("billion"),
            b"t" | b"T" | b"tn" => Some("trillion"),
            _ => None,
        };
        let mut clean = [0u8; 48];
        let mut n = 0;
        for &b in num {
            if b != b',' && n < clean.len() {
                clean[n] = b;
                n += 1;
            }
        }
        let num = &clean[..n];
        let (int, frac) = match num.iter().position(|&b| b == b'.') {
            Some(p) => (&num[..p], Some(&num[p + 1..])),
            None => (num, None),
        };
        if let Some(s) = scale {
            self.plain(num, Form::Cardinal, false);
            self.say(s);
            self.say(many);
            return;
        }
        let iv = number::parse(int).unwrap_or(0);
        match frac {
            Some(f) if f.len() == 2 && !sub1.is_empty() => {
                let fv = number::parse(f).unwrap_or(0);
                if iv > 0 || fv == 0 {
                    number::integer(if int.is_empty() { b"0" } else { int }, Form::Cardinal, self);
                    self.say(if iv == 1 { one } else { many });
                }
                if fv > 0 {
                    if iv > 0 {
                        self.say("and");
                    }
                    number::cardinal(fv, self);
                    self.say(if fv == 1 { sub1 } else { subn });
                }
            }
            Some(_) => {
                self.plain(num, Form::Cardinal, false);
                self.say(many);
            }
            None => {
                if iv > MAX_CARDINAL || int.is_empty() {
                    number::digits(int, self);
                } else {
                    number::integer(int, Form::Cardinal, self);
                }
                self.say(if iv == 1 { one } else { many });
            }
        }
        if scale.is_none() && !suffix.is_empty() {
            self.mixed(suffix);
        }
    }
}

/// "25", "25th", "25," as a day of the month.
fn day_number(c: &[u8]) -> Option<u8> {
    let d = c.iter().take_while(|b| is_digit(**b)).count();
    if d == 0 || d > 2 {
        return None;
    }
    let rest = &c[d..];
    let ord = rest.is_empty()
        || (rest.len() == 2
            && matches!(
                [rest[0].to_ascii_lowercase(), rest[1].to_ascii_lowercase()],
                [b's', b't'] | [b'n', b'd'] | [b'r', b'd'] | [b't', b'h']
            ));
    let v = number::parse(&c[..d])? as u8;
    (ord && (1..=31).contains(&v)).then_some(v)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn num_prefix(b: u8) -> bool {
    matches!(b, b'+' | b'-' | MINUS | b'~' | PLUSMINUS | b'#' | b'$' | EURO | POUND | YEN)
}

fn is_url(s: &[u8]) -> bool {
    if find(s, b"://").is_some() || (s.len() > 4 && lower_eq(&s[..4], "www.")) {
        return true;
    }
    // host.tld[/path]
    let host_end = s.iter().position(|&b| b == b'/').unwrap_or(s.len());
    let host = &s[..host_end];
    let Some(dot) = host.iter().rposition(|&b| b == b'.') else { return false };
    let tld = &host[dot + 1..];
    dot > 0
        && host.iter().all(|&b| is_alnum(b) || b == b'.' || b == b'-')
        && host[..dot].iter().any(|b| is_letter(*b))
        && !host.iter().any(|b| is_upper(*b))
        && TLDS.iter().any(|t| lower_eq(tld, t))
        && !host.contains(&b'_')
}

fn is_email(s: &[u8]) -> bool {
    let Some(at) = s.iter().position(|&b| b == b'@') else { return false };
    let (local, domain) = (&s[..at], &s[at + 1..]);
    !local.is_empty()
        && domain.contains(&b'.')
        && domain.first().is_some_and(|b| is_alnum(*b))
        && domain.last().is_some_and(|b| is_letter(*b))
        && local.iter().all(|&b| is_alnum(b) || b"._-+".contains(&b))
        && domain.iter().all(|&b| is_alnum(b) || b".-".contains(&b))
}

/// Names of symbols outside words.
fn symbol_name(b: u8) -> Option<&'static str> {
    Some(match b {
        EURO => "euros",
        POUND => "pounds",
        YEN => "yen",
        CENT => "cents",
        DEG => "degrees",
        COPY => "copyright",
        REG => "registered",
        TM => "trademark",
        SECT => "section",
        PARA => "paragraph",
        PLUSMINUS => "plus or minus",
        TIMES => "times",
        DIVIDE => "divided by",
        MICRO => "micro",
        HALF => "one half",
        QUARTER => "one quarter",
        THREEQ => "three quarters",
        OHM => "ohms",
        g if (GREEK..GREEK + 24).contains(&g) => fold::GREEK_NAMES[usize::from(g - GREEK)],
        _ => return None,
    })
}

/// Names of characters in spell mode.
fn spell_name(b: u8) -> Option<&'static str> {
    Some(match b {
        b'.' => "dot",
        b',' => "comma",
        b'-' => "dash",
        b'_' => "underscore",
        b'/' => "slash",
        b'\\' => "backslash",
        b':' => "colon",
        b';' => "semicolon",
        b'!' => "exclamation mark",
        b'?' => "question mark",
        b'@' => "at",
        b'#' => "hash",
        b'$' => "dollar",
        b'%' => "percent",
        b'^' => "caret",
        b'&' => "and",
        b'*' => "star",
        b'(' => "left paren",
        b')' => "right paren",
        b'[' => "left bracket",
        b']' => "right bracket",
        b'{' => "left brace",
        b'}' => "right brace",
        b'<' => "less than",
        b'>' => "greater than",
        b'=' => "equals",
        b'+' => "plus",
        b'~' => "tilde",
        b'`' => "backtick",
        b'\'' => "apostrophe",
        b'"' | LQUOTE | RQUOTE => "quote",
        b'|' => "bar",
        ELLIPSIS => "ellipsis",
        ENDASH | EMDASH | MINUS => "dash",
        BULLET => "bullet",
        _ => return symbol_name(b),
    })
}

impl Default for Normaliser {
    fn default() -> Normaliser {
        Normaliser::new(Options::default())
    }
}

impl Normaliser {
    /// A normaliser at the start of a text.
    pub const fn new(opts: Options) -> Normaliser {
        Normaliser { opts, st: State::new() }
    }

    /// The options.
    pub fn options(&self) -> Options {
        self.opts
    }

    /// Changes the options (from the next chunk on).
    pub fn set_options(&mut self, opts: Options) {
        self.opts = opts;
    }

    /// Back to the start of a text.
    pub fn reset(&mut self) {
        self.st = State::new();
    }

    /// A blank line: a paragraph pause (once).
    pub fn paragraph<S: Sink>(&mut self, sink: &mut S) {
        if self.st.any && self.st.last != Some(BreakKind::Paragraph) {
            let mut cx = Cx { sink, st: &mut self.st, opts: self.opts };
            cx.brk1(BreakKind::Paragraph);
        }
    }

    /// A single line end: a pause in line mode when the line did not end in
    /// punctuation.
    pub fn line<S: Sink>(&mut self, sink: &mut S) {
        if self.opts.lines && self.st.any && self.st.last.is_none() {
            let mut cx = Cx { sink, st: &mut self.st, opts: self.opts };
            cx.brk1(BreakKind::Line);
        }
    }

    /// Normalises one chunk (UTF-8 without white space, at most
    /// [`MAX_CHUNK`] bytes are looked at) with up to two following chunks
    /// as look-ahead; returns how many chunks it used (1–3).
    pub fn chunk<S: Sink>(&mut self, cur: &[u8], ahead: [Option<&[u8]>; 2], sink: &mut S) -> usize {
        let mut c = [0u8; MAX_CHUNK];
        let n = fold::fold(&cur[..cur.len().min(MAX_CHUNK)], &mut c);
        let mut a0 = [0u8; MAX_CHUNK];
        let mut a1 = [0u8; MAX_CHUNK];
        let l0 = ahead[0].map(|s| fold::fold(&s[..s.len().min(MAX_CHUNK)], &mut a0));
        let l1 = ahead[1].map(|s| fold::fold(&s[..s.len().min(MAX_CHUNK)], &mut a1));
        let la = [l0.map(|k| &a0[..k]), if l0.is_some() { l1.map(|k| &a1[..k]) } else { None }];
        let opts = self.opts;
        let mut cx = Cx { sink, st: &mut self.st, opts };
        let c = &c[..n];
        if cx.opts.spell {
            cx.spell(c);
            return 1;
        }
        let (lead, core, trail) = split(c);
        for &b in lead {
            cx.lead(b);
        }
        let r = cx.core(core, trail, la);
        if r.skip != usize::MAX {
            cx.trail(&trail[r.skip.min(trail.len())..]);
        }
        r.chunks.clamp(1, 1 + la.iter().filter(|x| x.is_some()).count())
    }
}

/// Test helpers: a sink collecting text.
#[cfg(any(test, feature = "build"))]
pub mod testing {
    extern crate std;
    use super::Sink;
    use crate::prosody::{BreakKind, Contour};
    use std::string::String;
    use std::vec::Vec;

    /// Collects words as text, letters upper case, breaks as marks.
    #[derive(Default, Debug)]
    pub struct Words(pub Vec<String>);

    impl Words {
        /// The collected items joined by spaces.
        pub fn text(&self) -> String {
            self.0.join(" ")
        }
    }

    pub use crate::prosody::mark;

    impl Sink for Words {
        fn word(&mut self, w: &[u8]) {
            self.0.push(String::from_utf8_lossy(w).to_lowercase());
        }

        fn letter(&mut self, c: u8) {
            self.0.push(String::from(char::from(c.to_ascii_uppercase())));
        }

        fn brk(&mut self, kind: BreakKind, contour: Contour) {
            self.0.push(String::from(mark(kind, contour)));
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn covered(phrase: &str) -> bool {
        phrase.split(' ').filter(|w| !w.is_empty()).all(|w| VOCABULARY.contains(&w))
    }

    #[test]
    fn vocabulary_covers_every_table() {
        let mut missing = std::vec::Vec::new();
        let mut want = |p: &'static str| {
            if !covered(p) {
                missing.push(p);
            }
        };
        number::ONES.iter().chain(&number::TENS[2..]).chain(&number::SCALES[1..]).for_each(|w| want(w));
        number::ORDINALS.iter().for_each(|(_, o)| want(o));
        number::DECADES.iter().for_each(|(_, p)| want(p));
        MONTHS.iter().for_each(|m| want(m));
        ABBREVIATIONS.iter().for_each(|(_, s, _)| want(s));
        UNITS.iter().for_each(|(_, s, p, _)| {
            want(s);
            want(p);
        });
        fold::GREEK_NAMES.iter().for_each(|g| want(g));
        for c in [Cur::Dollar, Cur::Euro, Cur::Pound, Cur::Yen, Cur::Cent] {
            let (a, b, c2, d) = c.words();
            [a, b, c2, d].into_iter().filter(|w| !w.is_empty()).for_each(&mut want);
        }
        for b in 0..=255u8 {
            if let Some(n) = spell_name(b) {
                want(n);
            }
        }
        for p in [
            "o'clock",
            "point",
            "minus",
            "plus",
            "about",
            "number",
            "percent",
            "half",
            "halves",
            "quarter",
            "quarters",
            "to",
            "by",
            "times",
            "dot",
            "and",
            "of",
            "the",
            "street",
            "for example",
            "that is",
            "slash",
            "dash",
            "underscore",
            "colon",
            "question mark",
            "equals",
            "hash",
            "tilde",
            "sharp",
            "capital",
            "less than",
            "greater than",
            "backslash",
            "dollars",
            "oh oh",
            "seconds",
            "second",
        ] {
            want(p);
        }
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(roman(b"IV"), Some(4));
        assert_eq!(roman(b"XIV"), Some(14));
        assert_eq!(roman(b"MCMXCIX"), Some(1999));
        assert_eq!(roman(b"IIII"), None);
        assert_eq!(roman(b"IC"), None);
        assert_eq!(roman(b""), None);
    }

    #[test]
    fn pronounceability() {
        assert!(pronounceable(b"strength") && pronounceable(b"rhythm") && pronounceable(b"NASA"));
        assert!(!pronounceable(b"xkcd") && !pronounceable(b"www") && !pronounceable(b"schmlt"));
    }
}
