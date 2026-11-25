//! Property tests (seeded xorshift, no extra dependencies): no panic on
//! arbitrary bytes and UTF-8, bounded output per input byte, the same
//! tokens whatever the feed sizes, well-formed tokens, and the duration
//! model's shape over every speed.

use orpheus_tts::duration::{self, Context, Speed};
use orpheus_tts::phoneme::{ALL, Class, Ph, Stress};
use orpheus_tts::prosody::KINDS;
use orpheus_tts::{Converter, Options, Token};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

/// Converts `text` feeding pieces of the sizes `piece` returns.
fn run(conv: &mut Converter, text: &[u8], mut piece: impl FnMut() -> usize) -> Vec<(Token, Vec<u8>)> {
    let mut v = Vec::new();
    let mut rest = text;
    loop {
        let k = piece().clamp(1, rest.len().max(1));
        let n = conv.feed(&rest[..k.min(rest.len())]);
        rest = &rest[n..];
        if rest.is_empty() {
            conv.finish();
        }
        while let Some(t) = conv.next_token() {
            let w = if matches!(t, Token::Word(_)) { conv.word_text().to_vec() } else { Vec::new() };
            v.push((t, w));
        }
        if rest.is_empty() {
            break;
        }
    }
    assert!(conv.is_done());
    v
}

fn check_tokens(text: &[u8], toks: &[(Token, Vec<u8>)]) {
    // Bounded: a byte says at most a short word or a symbol's name.
    assert!(toks.len() <= 24 * text.len() + 16, "{} tokens for {} bytes", toks.len(), text.len());
    let mut last_pos = 0;
    for (t, w) in toks {
        match t {
            Token::Word(word) => {
                assert!(word.pos as usize <= text.len());
                assert!(word.pos >= last_pos, "word positions go back");
                last_pos = word.pos;
                assert!(!w.is_empty() && w.len() <= 32);
            }
            Token::Phone(s) => {
                assert_eq!(s.phone.ph.is_vowel(), s.phone.stress != Stress::None);
                assert!(s.ms >= 1 && s.ms <= 2500, "{s:?}");
            }
            Token::Break(p) => assert!(p.ms <= 3000),
        }
    }
}

const WORDS: &[&str] = &[
    "the",
    "music",
    "player",
    "Chapter",
    "IV",
    "Henry",
    "VIII",
    "25",
    "Nov",
    "2026",
    "12:30",
    "7",
    "pm",
    "$3.50",
    "44.1",
    "kHz",
    "Mr.",
    "Smith",
    "etc.",
    "U.S.A.",
    "J.",
    "R.",
    "01",
    "-",
    "Artist",
    "Title.mp3",
    "x86",
    "USB",
    "https://example.com/a",
    "tim@example.com",
    "50%",
    "1,234,567",
    "3/4",
    "555-1234",
    "camelCase",
    "snake_case",
    "Beyoncé",
    "日本",
    "😀",
    "—",
    "...",
    "(aside)",
    "\"quoted\"",
    "Where",
    "is",
    "it?",
    "Stop!",
    "No.",
    "5",
    "café",
    "1990s",
    "'80s",
    "−3",
    "dB",
    "°C",
    "Ω",
    "½",
    "e.g.",
    "vs.",
    "St.",
    "Paul",
];

fn sentence(rng: &mut Rng) -> String {
    let n = 1 + rng.below(30);
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push_str(match rng.below(20) {
                0 => "\n",
                1 => "\n\n",
                2 => "  ",
                3 => "\t",
                _ => " ",
            });
        }
        s.push_str(rng.pick(WORDS));
        match rng.below(8) {
            0 => s.push(','),
            1 => s.push('.'),
            _ => {}
        }
    }
    s
}

#[test]
fn feed_sizes_do_not_matter() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for round in 0..300 {
        let text = sentence(&mut rng);
        let opts = Options { lines: round % 3 == 0, spell: round % 17 == 0 };
        let speed = Speed::from_quarters((round % 40) as u8 + 1);
        let whole = run(&mut Converter::new(opts, speed), text.as_bytes(), || usize::MAX);
        let ones = run(&mut Converter::new(opts, speed), text.as_bytes(), || 1);
        let mut r2 = Rng(round + 1);
        let random = run(&mut Converter::new(opts, speed), text.as_bytes(), || 1 + r2.below(64) as usize);
        assert_eq!(whole, ones, "{text:?}");
        assert_eq!(whole, random, "{text:?}");
        check_tokens(text.as_bytes(), &whole);
    }
}

#[test]
fn arbitrary_utf8_and_bytes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut conv = Converter::new(Options::default(), Speed::NORMAL);
    for round in 0..400 {
        let len = rng.below(600) as usize;
        let bytes: Vec<u8> = if round % 2 == 0 {
            (0..len).map(|_| rng.next() as u8).collect()
        } else {
            // Random characters from every plane, weighted to ASCII.
            let mut s = String::new();
            while s.len() < len {
                let c = match rng.below(6) {
                    0 => char::from_u32(rng.below(0x11_0000) as u32).unwrap_or('x'),
                    1 => char::from_u32(0x80 + rng.below(0x780) as u32).unwrap_or('x'),
                    _ => char::from(32 + rng.below(95) as u8),
                };
                s.push(c);
            }
            s.into_bytes()
        };
        conv.reset();
        conv.set_options(Options { spell: round % 5 == 0, lines: round % 3 == 0 });
        conv.set_speed(Speed::from_quarters((round % 40) as u8 + 1));
        let toks = run(&mut conv, &bytes, || 1 + (round as usize * 7) % 300);
        check_tokens(&bytes, &toks);
    }
}

#[test]
fn pathological_inputs() {
    let cases: Vec<Vec<u8>> = vec![
        vec![b' '; 5000],
        vec![b'\n'; 3000],
        vec![b'a'; 3000],
        vec![b'9'; 3000],
        b"%".repeat(1000),
        "?".repeat(1000).into_bytes(),
        "1,".repeat(1000).into_bytes(),
        "Ω".repeat(500).into_bytes(),
        "a ".repeat(2000).into_bytes(),
        [b"x".as_slice(), &[b' '; 300], b"y"].concat(),
        vec![0xF0, 0x9F, 0x98],
        vec![0xC3],
    ];
    for c in &cases {
        for spell in [false, true] {
            let mut conv = Converter::new(Options { spell, lines: true }, Speed::MAX);
            let toks = run(&mut conv, c, || 97);
            check_tokens(c, &toks);
        }
    }
}

#[test]
fn ram() {
    let n = std::mem::size_of::<Converter>();
    eprintln!("Converter: {n} B");
    assert!(n <= 1536, "{n}");
}

// ---------------------------------------------------------------- durations

fn contexts() -> Vec<Context> {
    let mut v = Vec::new();
    for bits in 0..64u32 {
        v.push(Context {
            word_initial: bits & 1 != 0,
            word_final_syllable: bits & 2 != 0,
            phrase_final_syllable: bits & 6 == 6,
            polysyllabic: bits & 8 != 0,
            next: [None, Some(Ph::T), Some(Ph::Z), Some(Ph::N)][(bits >> 4) as usize & 3],
            cluster: bits & 32 != 0,
        });
    }
    v
}

#[test]
fn durations_fall_with_speed_and_keep_floors() {
    for ph in ALL {
        for st in [Stress::Primary, Stress::Secondary, Stress::Unstressed] {
            let st = if ph.is_vowel() { st } else { Stress::None };
            for c in contexts() {
                let mut prev = u16::MAX;
                let at1 = duration::duration_ms(ph, st, &c, Speed::NORMAL);
                for q in 1..=40u8 {
                    let d = duration::duration_ms(ph, st, &c, Speed::from_quarters(q));
                    assert!(d <= prev, "{ph:?} {st:?} {c:?}: {d} ms at {q}/4 after {prev}");
                    let floor = duration::floor_at(ph.class(), Speed::from_quarters(q)).min(u32::from(at1));
                    assert!(u32::from(d) >= floor, "{ph:?} {q}: {d}");
                    prev = d;
                }
            }
        }
    }
}

#[test]
fn slow_speech_lengthens_vowels_not_stops() {
    let c = Context { word_initial: true, word_final_syllable: true, ..Context::default() };
    let quarter = Speed::from_quarters(1);
    for ph in ALL {
        let st = if ph.is_vowel() { Stress::Primary } else { Stress::None };
        let d1 = f64::from(duration::duration_ms(ph, st, &c, Speed::NORMAL));
        let d025 = f64::from(duration::duration_ms(ph, st, &c, quarter));
        let r = d025 / d1;
        match ph.class() {
            Class::Vowel | Class::Diphthong => assert!((3.9..=4.1).contains(&r), "{ph:?} {r}"),
            Class::Stop | Class::Fricative | Class::Affricate | Class::Aspirate => {
                assert!((1.6..=1.8).contains(&r), "{ph:?} {r}");
            }
            _ => assert!((2.4..=2.6).contains(&r), "{ph:?} {r}"),
        }
    }
}

#[test]
fn obstruents_compress_less_than_vowels() {
    let c =
        Context { word_initial: true, word_final_syllable: true, polysyllabic: true, ..Context::default() };
    for q in [8u8, 16, 24, 40] {
        let s = Speed::from_quarters(q);
        let ratio = |ph: Ph, st| {
            f64::from(duration::duration_ms(ph, st, &c, s))
                / f64::from(duration::duration_ms(ph, st, &c, Speed::NORMAL))
        };
        let vowel = ratio(Ph::AA, Stress::Primary);
        for ph in [Ph::P, Ph::T, Ph::K, Ph::S, Ph::SH, Ph::F, Ph::CH] {
            assert!(ratio(ph, Stress::None) > vowel, "{ph:?} at {q}/4");
        }
    }
}

#[test]
fn pauses_compress_faster_than_speech_above_2x() {
    let c = Context { word_initial: true, word_final_syllable: true, ..Context::default() };
    for k in KINDS.iter().filter(|k| k.base_ms() > 0) {
        for q in 9..=40u8 {
            let s = Speed::from_quarters(q);
            let p = f64::from(duration::pause_ms(*k, s)) / f64::from(duration::pause_ms(*k, Speed::NORMAL));
            let v = f64::from(duration::duration_ms(Ph::AA, Stress::Primary, &c, s))
                / f64::from(duration::duration_ms(Ph::AA, Stress::Primary, &c, Speed::NORMAL));
            assert!(p < v || duration::pause_ms(*k, s) <= 30, "{k:?} at {q}/4: pause {p:.3} speech {v:.3}");
        }
        for q in 1..=8u8 {
            let want = f64::from(k.base_ms()) * 4.0 / f64::from(q);
            let got = f64::from(duration::pause_ms(*k, Speed::from_quarters(q)));
            assert!((got - want).abs() <= 1.0 || got == 30.0, "{k:?} at {q}/4: {got} vs {want}");
        }
    }
}

#[test]
fn vowel_reduction_and_flattening_above_4x() {
    for q in 1..=40u8 {
        let s = Speed::from_quarters(q);
        for ph in ALL {
            let r = duration::reduce(ph, Stress::Unstressed, s);
            assert_eq!(duration::reduce(ph, Stress::Primary, s), ph);
            if q <= 16 || !ph.is_vowel() {
                assert_eq!(r, ph);
            } else {
                assert!(matches!(r, Ph::AH | Ph::IH | Ph::ER), "{ph:?} -> {r:?}");
            }
        }
        let depth = duration::pitch_depth_pct(s);
        assert!((40..=100).contains(&depth));
        if q > 16 {
            assert!(depth < 100);
        }
    }
    // The converter applies it: at 10× no unstressed full vowel is left.
    let mut conv = Converter::new(Options::default(), Speed::MAX);
    conv.convert(b"The information about photography was available yesterday.", |_, t| {
        if let Token::Phone(s) = t
            && s.phone.stress == Stress::Unstressed
        {
            assert!(matches!(s.phone.ph, Ph::AH | Ph::IH | Ph::ER), "{s:?}");
        }
    });
}

#[test]
fn speed_changes_durations_end_to_end() {
    let text = b"Albums, three of twelve. Battery at fifty percent.";
    let total = |q: u8| {
        let mut conv = Converter::new(Options::default(), Speed::from_quarters(q));
        let mut ms = 0u32;
        conv.convert(text, |_, t| match t {
            Token::Phone(s) => ms += u32::from(s.ms),
            Token::Break(p) => ms += u32::from(p.ms),
            Token::Word(_) => {}
        });
        ms
    };
    let t: Vec<u32> = [1u8, 2, 4, 8, 16, 40].iter().map(|&q| total(q)).collect();
    eprintln!("utterance ms at 0.25/0.5/1/2/4/10x: {t:?}");
    assert!(t.windows(2).all(|w| w[0] > w[1]), "{t:?}");
    // About 4 s at 1×; the 10× setting is at least 7× shorter (the floors
    // keep it from a full 10×).
    assert!((2500..=4500).contains(&t[2]), "{t:?}");
    assert!(t[5] * 7 <= t[2], "{t:?}");
}

/// Host throughput (`cargo test --release -p orpheus-tts --test properties
/// -- --ignored --nocapture`); the player's number is TTS-2's to measure.
#[test]
#[ignore]
fn throughput() {
    let mut rng = Rng(7);
    let text: String = (0..2000).map(|_| sentence(&mut rng) + " ").collect();
    let start = std::time::Instant::now();
    let (mut words, mut ms) = (0u64, 0u64);
    let mut conv = Converter::new(Options::default(), Speed::NORMAL);
    conv.convert(text.as_bytes(), |_, t| match t {
        Token::Word(_) => words += 1,
        Token::Phone(s) => ms += u64::from(s.ms),
        Token::Break(p) => ms += u64::from(p.ms),
    });
    let el = start.elapsed().as_secs_f64();
    eprintln!(
        "{} bytes, {words} words in {:.1} ms: {:.0} words/s, {:.1} us/word; speech at 1x {:.0} s ({:.0}x real time)",
        text.len(),
        el * 1e3,
        words as f64 / el,
        el * 1e6 / words as f64,
        ms as f64 / 1e3,
        ms as f64 / 1e3 / el
    );
}
