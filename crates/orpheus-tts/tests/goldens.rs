//! Phoneme-string goldens: words, numbers, dates, names, file names and
//! sentences to ARPAbet with stress (`tests/goldens/phonemes.txt`), and the
//! duration tables at 0.25×, 1×, 4× and 10× (`tests/goldens/durations.txt`).
//! `ORPHEUS_WRITE_GOLDENS=1 cargo test -p orpheus-tts --test goldens`
//! rewrites both; look at the diff.

use orpheus_tts::duration::{self, Context, Speed};
use orpheus_tts::phoneme::{ALL, Stress};
use orpheus_tts::prosody::{KINDS, mark};
use orpheus_tts::{Converter, Options, Token};
use std::fmt::Write as _;
use std::path::PathBuf;

/// The inputs: one per line.
const INPUTS: &[&str] = &[
    // common words
    "the",
    "a",
    "and",
    "of",
    "to",
    "in",
    "is",
    "you",
    "that",
    "it",
    "he",
    "was",
    "for",
    "on",
    "are",
    "with",
    "they",
    "be",
    "at",
    "one",
    "have",
    "this",
    "from",
    "word",
    "but",
    "what",
    "some",
    "we",
    "can",
    "out",
    "other",
    "were",
    "all",
    "there",
    "when",
    "up",
    "use",
    "your",
    "how",
    "said",
    "each",
    "she",
    "which",
    "people",
    "water",
    "through",
    "thought",
    "though",
    "enough",
    "laughed",
    "women",
    "island",
    "colonel",
    "music",
    "player",
    "battery",
    "settings",
    "volume",
    "playlist",
    "shuffle",
    "repeat",
    "album",
    "artist",
    "bluetooth",
    "headphones",
    "recording",
    "radio",
    "library",
    "favourites",
    "brightness",
    "language",
    "information",
    "photograph",
    "photography",
    "nation",
    "national",
    "record",
    "present",
    "beautiful",
    "unbelievable",
    "walked",
    "walking",
    "dogs",
    "cats",
    "churches",
    "wanted",
    "happily",
    "biggest",
    // numbers
    "0",
    "13",
    "42",
    "100",
    "1,234",
    "1,000,000",
    "3.14",
    "-7",
    "21st",
    "1984",
    "2026",
    "1/2",
    "50%",
    // dates and times
    "2026-11-25",
    "25 Nov 2026",
    "November 25th",
    "12:30",
    "7:05pm",
    // currency and units
    "$3.50",
    "€20",
    "44.1 kHz",
    "320kbps",
    "5 MB",
    "-3 dB",
    // names
    "Orpheus",
    "Eurydice",
    "Beethoven",
    "Mozart",
    "Tchaikovsky",
    "Beyoncé",
    "Björk",
    "Smith",
    "Jones",
    "Elizabeth",
    "Henry VIII",
    "Chapter IV",
    "Mr. Smith",
    "Dr. Jones",
    "J. R. R. Tolkien",
    // file names, acronyms, addresses
    "01 - Artist - Title.mp3",
    "02_Some_Song.flac",
    "track01.ogg",
    "myFileName.txt",
    "USB",
    "MP3",
    "NASA",
    "https://example.com",
    "tim@example.com",
    // sentences
    "Hello, world.",
    "Where are you going?",
    "Is it ready?",
    "Stop!",
    "Albums, 3 of 12.",
    "Battery low: 15%.",
    "Now playing: Moonlight Sonata (1st movement) by Beethoven.",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn render(text: &str) -> String {
    let mut c = Converter::new(Options::default(), Speed::NORMAL);
    let mut out = String::new();
    c.convert(text.as_bytes(), |_, t| match t {
        Token::Word(_) => {
            if !out.is_empty() {
                out.push_str(" /");
            }
        }
        Token::Phone(s) => {
            out.push(' ');
            out.push_str(s.phone.ph.name());
            if s.phone.stress != Stress::None {
                out.push(char::from(b'0' + s.phone.stress as u8));
            }
        }
        Token::Break(p) => {
            let _ = write!(out, " {}", mark(p.kind, p.contour));
        }
    });
    out.trim().to_string()
}

fn compare(name: &str, text: String) {
    let path = root().join("tests").join("goldens").join(name);
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut bad = Vec::new();
    for (w, g) in want.lines().zip(text.lines()) {
        if w != g {
            bad.push(format!("  want {w}\n  got  {g}"));
        }
    }
    assert_eq!(want.lines().count(), text.lines().count(), "{name}: line count");
    assert!(bad.is_empty(), "{name}: {} lines differ:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn phonemes() {
    let mut text = String::new();
    for i in INPUTS {
        let _ = writeln!(text, "{i}\t{}", render(i));
    }
    compare("phonemes.txt", text);
}

#[test]
fn durations() {
    let mut text = String::from(
        "# phone stress | ms at 0.25x 1x 4x 10x: in a stressed, word-initial, non-final position | the same, phrase-final\n",
    );
    let speeds = [1u8, 4, 16, 40].map(Speed::from_quarters);
    for ph in ALL {
        let stresses: &[Stress] =
            if ph.is_vowel() { &[Stress::Primary, Stress::Unstressed] } else { &[Stress::None] };
        for &st in stresses {
            let mid = Context { word_initial: true, polysyllabic: true, ..Context::default() };
            let fin = Context { word_final_syllable: true, phrase_final_syllable: true, ..mid };
            let row = |c: &Context| speeds.map(|s| duration::duration_ms(ph, st, c, s));
            let _ = writeln!(text, "{:<3} {} | {:?} | {:?}", ph.name(), st as u8, row(&mid), row(&fin));
        }
    }
    text.push_str("# pause | ms at 0.25x 1x 2x 4x 10x\n");
    for k in KINDS {
        let ms = [1u8, 4, 8, 16, 40].map(|q| duration::pause_ms(k, Speed::from_quarters(q)));
        let _ = writeln!(text, "{:<11} {:?}", format!("{k:?}"), ms);
    }
    compare("durations.txt", text);
}
