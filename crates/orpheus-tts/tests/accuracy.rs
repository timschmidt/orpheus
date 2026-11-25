//! Letter-to-sound accuracy against CMUdict on the committed fixture
//! `data/eval.tsv` (written by `cargo xtask tts-lexicon`: the 1,500 most
//! frequent words of the Gutenberg count, every 20th word of ranks
//! 1,501–40,000 with stride 20 so that weighted sums estimate the whole
//! corpus, and the forced vocabulary with stride 0).
//!
//! Right means: the phonemes equal one of CMUdict's variants and the primary
//! stress is on the same vowel (one-vowel words: phonemes only). The
//! thresholds sit a little under the measured values (04 §8 "as built").

use orpheus_tts::lexicon::{Entry, Lexicon};
use orpheus_tts::phoneme::{Phone, Stress};
use orpheus_tts::words::{MAX_WORD_PHONES, by_rules, pronounce};

struct Row {
    word: String,
    count: u64,
    stride: u64,
    variants: Vec<Vec<Phone>>,
}

fn fixture() -> Vec<Row> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join("eval.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 4, "{l}");
            Row {
                word: f[0].to_string(),
                count: f[1].parse().unwrap(),
                stride: f[2].parse().unwrap(),
                variants: f[3]
                    .split('|')
                    .map(|v| v.split(' ').map(|t| Phone::parse(t.as_bytes()).unwrap()).collect())
                    .collect(),
            }
        })
        .collect()
}

fn key(p: &[Phone]) -> (Vec<u8>, Option<usize>, usize) {
    let vowels: Vec<&Phone> = p.iter().filter(|x| x.ph.is_vowel()).collect();
    (
        p.iter().map(|x| x.ph as u8).collect(),
        vowels.iter().position(|x| x.stress == Stress::Primary),
        vowels.len(),
    )
}

/// (phonemes right, phonemes and primary stress right)
fn judge(got: &[Phone], variants: &[Vec<Phone>]) -> (bool, bool) {
    let (g, gs, gv) = key(got);
    let mut ph = false;
    for v in variants {
        let (r, rs, _) = key(v);
        if r == g {
            ph = true;
            if gv <= 1 || gs == rs {
                return (true, true);
            }
        }
    }
    (ph, false)
}

#[derive(Default, Debug)]
struct Acc {
    words: f64,
    ph_words: f64,
    ok_words: f64,
    tokens: f64,
    ph_tokens: f64,
    ok_tokens: f64,
}

impl Acc {
    fn add(&mut self, r: &Row, (ph, ok): (bool, bool)) {
        let w = r.stride as f64;
        let t = (r.count * r.stride) as f64;
        self.words += w;
        self.tokens += t;
        if ph {
            self.ph_words += w;
            self.ph_tokens += t;
        }
        if ok {
            self.ok_words += w;
            self.ok_tokens += t;
        }
    }

    fn pct(a: f64, b: f64) -> f64 {
        100.0 * a / b
    }
}

fn measure(rows: &[Row], f: impl Fn(&str, &mut [Phone]) -> usize) -> (Acc, Acc) {
    let (mut top, mut all) = (Acc::default(), Acc::default());
    let mut buf = [Phone::new(orpheus_tts::Ph::AH, Stress::None); MAX_WORD_PHONES];
    for r in rows.iter().filter(|r| r.stride > 0) {
        let n = f(&r.word, &mut buf);
        let j = judge(&buf[..n], &r.variants);
        if r.stride == 1 {
            top.add(r, j);
        }
        all.add(r, j);
    }
    (top, all)
}

#[test]
fn rules_alone() {
    let rows = fixture();
    let (top, all) = measure(&rows, |w, out| by_rules(w.as_bytes(), out));
    eprintln!(
        "rules alone: top 1500 words {:.1} % (phonemes {:.1} %); all words {:.1} % (phonemes {:.1} %); tokens {:.1} % (phonemes {:.1} %)",
        Acc::pct(top.ok_words, top.words),
        Acc::pct(top.ph_words, top.words),
        Acc::pct(all.ok_words, all.words),
        Acc::pct(all.ph_words, all.words),
        Acc::pct(all.ok_tokens, all.tokens),
        Acc::pct(all.ph_tokens, all.tokens),
    );
    // NRL rules on CMUdict: about 40 % of word types, 80 % of tokens.
    assert!(Acc::pct(all.ph_words, all.words) > 36.0);
    assert!(Acc::pct(all.ph_tokens, all.tokens) > 78.0);
    assert!(Acc::pct(all.ok_tokens, all.tokens) > 76.5);
    assert!(Acc::pct(top.ph_words, top.words) > 58.0);
}

#[test]
fn rules_and_lexicon() {
    let rows = fixture();
    let lex = Lexicon::builtin();
    let (top, all) = measure(&rows, |w, out| pronounce(&lex, w.as_bytes(), out).0);
    eprintln!(
        "rules + lexicon: top 1500 words {:.2} %; all words {:.1} %; tokens {:.2} % (phonemes {:.2} %)",
        Acc::pct(top.ok_words, top.words),
        Acc::pct(all.ok_words, all.words),
        Acc::pct(all.ok_tokens, all.tokens),
        Acc::pct(all.ph_tokens, all.tokens),
    );
    assert!(Acc::pct(top.ok_words, top.words) > 99.5);
    assert!(Acc::pct(all.ok_tokens, all.tokens) > 97.5);
}

#[test]
fn forced_vocabulary_is_right() {
    // The normaliser's own words and the interface's words: all right.
    let lex = Lexicon::builtin();
    let mut buf = [Phone::new(orpheus_tts::Ph::AH, Stress::None); MAX_WORD_PHONES];
    let mut wrong = Vec::new();
    let mut n = 0;
    for r in fixture().iter().filter(|r| r.stride == 0) {
        let k = pronounce(&lex, r.word.as_bytes(), &mut buf).0;
        n += 1;
        if !judge(&buf[..k], &r.variants).1 {
            wrong.push(r.word.clone());
        }
    }
    assert!(n > 500, "{n}");
    assert!(wrong.is_empty(), "{} of {n} wrong: {wrong:?}", wrong.len());
}

#[test]
fn every_normaliser_word_is_in_the_fixture() {
    // Words the normaliser says that CMUdict lacks are left to the rules;
    // list them so a change is noticed.
    let rows = fixture();
    let missing: Vec<&str> = orpheus_tts::normalise::VOCABULARY
        .iter()
        .copied()
        .filter(|w| !rows.iter().any(|r| r.word == *w))
        .collect();
    assert!(missing.is_empty(), "not in the fixture: {missing:?}");
}

#[test]
fn lexicon_entries_round_trip() {
    let lex = Lexicon::builtin();
    let mut n = 0;
    let mut out = [Phone::new(orpheus_tts::Ph::AH, Stress::None); 48];
    lex.for_each(|w, e, ph| {
        n += 1;
        let got = lex.lookup(w, &mut out).unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(w)));
        assert_eq!(got, e);
        if let Entry::Full(k) = got {
            assert_eq!(&out[..k], ph);
        }
    });
    assert_eq!(n, lex.len());
    assert!(orpheus_tts::lexicon::DATA.len() <= 32 * 1024, "lexicon over its 32 KiB budget");
}
