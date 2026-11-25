//! Normalisation: what the converter says for written forms (words as
//! text, spelled letters in capitals, breaks as `prosody::mark`s).

use orpheus_tts::prosody::mark;
use orpheus_tts::{Converter, Options, Speed, Token};

fn say_with(opts: Options, text: &str) -> String {
    let mut c = Converter::new(opts, Speed::NORMAL);
    let mut out: Vec<String> = Vec::new();
    c.convert(text.as_bytes(), |c, t| match t {
        Token::Word(w) => {
            let s = String::from_utf8_lossy(c.word_text()).into_owned();
            out.push(if w.spelled { s.to_uppercase() } else { s.to_lowercase() });
        }
        Token::Break(p) => out.push(mark(p.kind, p.contour).to_string()),
        Token::Phone(_) => {}
    });
    assert_eq!(c.dropped(), 0, "{text}");
    out.join(" ")
}

fn say(text: &str) -> String {
    say_with(Options::default(), text)
}

#[track_caller]
fn check(cases: &[(&str, &str)]) {
    let mut bad = Vec::new();
    for (text, want) in cases {
        let got = say(text);
        if got != *want {
            bad.push(format!("  {text:?}\n    want {want:?}\n    got  {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{} of {} wrong:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

#[test]
fn cardinals_and_signs() {
    check(&[
        ("0", "zero"),
        ("7", "seven"),
        ("42", "forty two"),
        ("100", "one hundred"),
        ("101", "one hundred one"),
        ("1,234", "one thousand two hundred thirty four"),
        ("1,000,000", "one million"),
        (
            "123456789",
            "one hundred twenty three million four hundred fifty six thousand seven hundred eighty nine",
        ),
        ("-5", "minus five"),
        ("−12", "minus twelve"),
        ("+3", "plus three"),
        ("~20", "about twenty"),
        ("#1", "number one"),
        ("3.14", "three point one four"),
        (".5", "point five"),
        ("0.05", "zero point zero five"),
        ("1.2.3", "one point two point three"),
        ("192.168.0.1", "one hundred ninety two dot one hundred sixty eight dot zero dot one"),
    ]);
}

#[test]
fn ordinals_years_digit_strings() {
    check(&[
        ("1st", "first"),
        ("2nd", "second"),
        ("23rd", "twenty third"),
        ("101st", "one hundred first"),
        ("12th", "twelfth"),
        ("1984", "nineteen eighty four"),
        ("2026", "twenty twenty six"),
        ("2005", "two thousand five"),
        ("1900", "nineteen hundred"),
        ("1990s", "nineteen nineties"),
        ("'80s", "eighties"),
        ("007", "zero zero seven"),
        ("01", "one"),
        ("1234567890123456", "one two three four five six seven eight nine zero one two three four five six"),
        ("555-1234", "five five five | one two three four"),
        ("1-800-555-0199", "one | eight zero zero | five five five | zero one nine nine"),
        ("10-20", "ten to twenty"),
        ("1990-1995", "nineteen ninety to nineteen ninety five"),
        ("3/4", "three quarters"),
        ("1/2", "one half"),
        ("2/3", "two thirds"),
        ("24/7", "twenty four seven"),
        ("1920x1080", "one thousand nine hundred twenty by one thousand eighty"),
    ]);
}

#[test]
fn times_and_dates() {
    check(&[
        ("12:30", "twelve thirty"),
        ("7:05", "seven oh five"),
        ("7:00", "seven o'clock"),
        ("7pm", "seven P M"),
        ("7 pm", "seven P M"),
        ("7:30pm", "seven thirty P M"),
        ("10 a.m.", "ten A M"),
        ("11:15 AM", "eleven fifteen A M"),
        ("1:02:03", "one oh two and three seconds"),
        ("2026-11-25", "november twenty fifth twenty twenty six"),
        ("11/25/2026", "november twenty fifth twenty twenty six"),
        ("25/11/2026", "the twenty fifth of november twenty twenty six"),
        ("25 Nov 2026", "the twenty fifth of november twenty twenty six"),
        ("25 November", "the twenty fifth of november"),
        ("November 25, 2026", "november twenty fifth twenty twenty six"),
        ("Nov. 25th", "november twenty fifth"),
        ("May 5", "may fifth"),
        ("I may 5", "i may five"),
        ("On 4 July 1776.", "on the fourth of july seventeen seventy six ."),
    ]);
}

#[test]
fn money_percent_units() {
    check(&[
        ("$3.50", "three dollars and fifty cents"),
        ("$1", "one dollar"),
        ("$0.99", "ninety nine cents"),
        ("$1,000", "one thousand dollars"),
        ("$1.5M", "one point five million dollars"),
        ("€20", "twenty euros"),
        ("20€", "twenty euros"),
        ("£5.01", "five pounds and one penny"),
        ("¥100", "one hundred yen"),
        ("-$5", "minus five dollars"),
        ("50%", "fifty percent"),
        ("3.5 %", "three point five percent"),
        ("44.1 kHz", "forty four point one kilohertz"),
        ("44.1kHz", "forty four point one kilohertz"),
        ("320kbps", "three hundred twenty kilobits per second"),
        ("1 MB", "one megabyte"),
        ("5 MB", "five megabytes"),
        ("10ms", "ten milliseconds"),
        ("-3 dB", "minus three decibels"),
        ("20°C", "twenty degrees celsius"),
        ("1 °C", "one degree celsius"),
        ("5m", "five meters"),
        ("5 m", "five M"),
        ("4000mAh", "four thousand milliamp hours"),
        ("2x", "two times"),
        ("4K", "four K"),
        ("3D", "three D"),
    ]);
}

#[test]
fn abbreviations_initials_roman() {
    check(&[
        ("Mr. Smith", "mister smith"),
        ("Dr. Who", "doctor who"),
        ("Dr Jones", "doctor jones"),
        ("St. Paul", "saint paul"),
        ("Baker St. is", "baker street is"),
        ("apples, pears, etc. and more", "apples , pears , et cetera and more"),
        ("Cats etc. Dogs", "cats et cetera . dogs"),
        ("cats vs. dogs", "cats versus dogs"),
        ("e.g. this", "for example this"),
        ("i.e. that", "that is that"),
        ("No. 5", "number five"),
        ("No.", "no ."),
        ("J. R. R. Tolkien", "J R R tolkien"),
        ("U.S.A.", "U S A ."),
        ("the U.S. army", "the U S army"),
        ("Plan B.", "plan B ."),
        ("Chapter IV", "chapter four"),
        ("Part II.", "part two ."),
        ("Henry VIII", "henry the eighth"),
        ("Louis XIV was", "louis the fourteenth was"),
        ("I am", "i am"),
        ("World War II", "world war two"),
        ("Wed. 25", "wednesday twenty five"),
    ]);
}

#[test]
fn words_acronyms_spelling() {
    check(&[
        ("USB", "U S B"),
        ("NASA", "nasa"),
        ("THE BEST", "the best"),
        ("MP3", "M P three"),
        ("x86", "X eighty six"),
        ("iPhone", "i phone"),
        ("XMLHttpRequest", "X M L H T T P request"),
        ("camelCaseWord", "camel case word"),
        ("snake_case_name", "snake case name"),
        ("well-known", "well known"),
        ("C#", "C sharp"),
        ("rock & roll", "rock and roll"),
        ("A cat", "a cat"),
        ("xkcd", "X K C D"),
        ("McDonald", "mcdonald"),
        ("don't", "don't"),
        ("rock 'n' roll", "rock N roll"),
    ]);
}

#[test]
fn file_names() {
    check(&[
        ("01 - Artist - Title.mp3", "one - artist - title"),
        ("02_Some_Song.flac", "two some song"),
        ("track01.ogg", "track one"),
        ("myFile_v2.txt", "my file V two"),
        ("Beyoncé - Halo.opus", "beyonce - halo"),
        ("notes.TXT", "notes"),
        ("archive.tar", "archive dot tar"),
    ]);
}

#[test]
fn urls_and_emails() {
    check(&[
        ("https://example.com/path", "example dot com slash path"),
        ("www.orpheus.dev", "W W W dot orpheus dot dev"),
        ("example.io", "example dot I O"),
        ("ftp://host.org", "F T P colon slash slash host dot org"),
        ("tim@example.com", "tim at example dot com"),
        ("first.last@mail.co.uk", "first dot last at mail dot co dot U K"),
        ("https://x.com/a?b=1", "X dot com slash A question mark B equals one"),
    ]);
}

#[test]
fn punctuation_and_intonation() {
    check(&[
        ("Hello, world.", "hello , world ."),
        ("Is it here?", "is it here ?/"),
        ("Where is it?", "where is it ?\\"),
        ("Stop!", "stop !"),
        ("Wait...", "wait ..."),
        ("Wait… what", "wait ... what"),
        ("Note: this", "note : this"),
        ("one; two", "one ; two"),
        ("a (small) cat", "a ( small ) cat"),
        ("\"Hi,\" she said.", "\" hi , \" she said ."),
        ("“Yes”", "\" yes \""),
        ("so — then", "so - then"),
        ("Really?!", "really ?/"),
        ("one\n\ntwo", "one ¶ two"),
        ("one\ntwo", "one two"),
    ]);
}

#[test]
fn line_mode() {
    let o = Options { lines: true, ..Options::default() };
    assert_eq!(say_with(o, "Albums\nArtists\nSongs."), "albums / artists / songs .");
    assert_eq!(say_with(o, "Albums.\nArtists"), "albums . artists");
}

#[test]
fn other_scripts_and_symbols() {
    check(&[
        ("日本語 text", "text"),
        ("Привет world", "world"),
        ("emoji 😀 here", "emoji here"),
        ("α and Ω", "alpha and omega"),
        ("10 Ω", "ten ohms"),
        ("©2026", "copyright twenty twenty six"),
        ("5 × 3", "five times three"),
        ("½", "one half"),
        ("café", "cafe"),
        ("naïve", "naive"),
        ("Motörhead", "motorhead"),
    ]);
}

#[test]
fn spell_mode() {
    let o = Options { spell: true, ..Options::default() };
    assert_eq!(say_with(o, "Ab1-c"), "capital A B one dash C |");
    assert_eq!(say_with(o, "a.b c"), "A dot B | C |");
}
