//! The lexicon tool behind `cargo xtask tts-lexicon` (`build` feature,
//! std): fetches CMUdict and a word-frequency corpus, measures the rules
//! against CMUdict, chooses the exception entries by frequency within the
//! size budget, writes `src/lexicon.bin` and the test fixture
//! `data/eval.tsv`, and reports the accuracy/size curve.
//!
//! # Data
//!
//! * CMUdict (BSD-style licence, `THIRD-PARTY.md`), pinned by commit and
//!   SHA-256.
//! * Frequencies: word counts over public-domain Project Gutenberg books
//!   ([`BOOKS`], from a Gutenberg mirror; counts are facts, nothing of the
//!   texts is kept), plus two forced vocabularies with a high weight: the
//!   words the normaliser itself says ([`crate::normalise::VOCABULARY`])
//!   and the words of the user interface's strings (read from
//!   `crates/orpheus-apps/src` and `crates/orpheus-ui/src`).
//!
//! # Correctness
//!
//! A pronunciation is right when its phonemes equal one of CMUdict's
//! variants and its primary stress sits on the same vowel (words of one
//! vowel: phonemes only). Secondary stress is not compared.

extern crate std;

use crate::lexicon::build::{Pron, encode};
use crate::lexicon::{Entry, Lexicon, MAX_WORD};
use crate::phoneme::{Phone, Stress};
use crate::words::{Lookup, MAX_WORD_PHONES, by_rules, pronounce};
use std::collections::{BTreeMap, HashMap};
use std::format;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::{String, ToString};
use std::vec::Vec;

/// CMUdict at a fixed commit.
pub const CMUDICT_URL: &str = "https://raw.githubusercontent.com/cmusphinx/cmudict/74790861f652b15e4ac49015a90074ad62a27690/cmudict.dict";
/// SHA-256 of that file.
pub const CMUDICT_SHA256: &str = "81917843c7f44ce2b094ac63873c2c7a4cf802040792c455ba3ca406891c3d22";
/// A Project Gutenberg mirror (the main site may refuse automated clients).
pub const MIRROR: &str = "https://aleph.pglaf.org";
/// The books counted (Project Gutenberg numbers; all public domain in the
/// US): novels, stories, plays in prose, essays, philosophy, history,
/// economics, travel and children's books from 1600–1925.
pub const BOOKS: [u32; 57] = [
    1342, 11, 84, 2701, 1661, 98, 1400, 76, 74, 345, 174, 1260, 768, 2600, 1184, 5200, 64317, 120, 35, 36,
    43, 219, 205, 158, 161, 730, 2814, 244, 2852, 55, 16, 45, 1497, 147, 3300, 20203, 23, 2554, 996, 25344,
    1023, 103, 164, 160, 514, 215, 910, 5827, 2680, 4300, 1952, 2591, 6130, 1727, 8800, 3207, 1232,
];
/// Pronunciations written for this project (not from CMUdict) of words
/// the normaliser says that CMUdict lacks; added to the dictionary and
/// forced like the rest of the normaliser's vocabulary.
pub const EXTRA: [(&str, &str); 30] = [
    ("zeroth", "Z IH1 R OW0 TH"),
    ("trillionth", "T R IH1 L Y AH0 N TH"),
    ("kilohertz", "K IH1 L AH0 HH ER2 T S"),
    ("gigahertz", "G IH1 G AH0 HH ER2 T S"),
    ("kibibyte", "K IH1 B IY0 B AY2 T"),
    ("kibibytes", "K IH1 B IY0 B AY2 T S"),
    ("mebibyte", "M EH1 B IY0 B AY2 T"),
    ("mebibytes", "M EH1 B IY0 B AY2 T S"),
    ("gibibyte", "G IH1 B IY0 B AY2 T"),
    ("gibibytes", "G IH1 B IY0 B AY2 T S"),
    ("kilobits", "K IH1 L AH0 B IH2 T S"),
    ("megabits", "M EH1 G AH0 B IH2 T S"),
    ("microsecond", "M AY1 K R OW0 S EH2 K AH0 N D"),
    ("microseconds", "M AY1 K R OW0 S EH2 K AH0 N D Z"),
    ("milliamp", "M IH1 L IY0 AE2 M P"),
    ("milliamps", "M IH1 L IY0 AE2 M P S"),
    ("millivolt", "M IH1 L IY0 V OW2 L T"),
    ("millivolts", "M IH1 L IY0 V OW2 L T S"),
    ("milliwatt", "M IH1 L IY0 W AA2 T"),
    ("milliwatts", "M IH1 L IY0 W AA2 T S"),
    ("miz", "M IH1 Z"),
    ("semicolon", "S EH1 M IY0 K OW2 L AH0 N"),
    ("caret", "K EH1 R AH0 T"),
    ("paren", "P ER0 EH1 N"),
    ("backtick", "B AE1 K T IH2 K"),
    ("ellipsis", "IH0 L IH1 P S IH0 S"),
    ("eta", "EY1 T AH0"),
    ("omicron", "AA1 M AH0 K R AA2 N"),
    ("tau", "T AW1"),
    ("upsilon", "UW1 P S AH0 L AA2 N"),
];

/// Size budget of `lexicon.bin` (04 §8: ≤ 32 KiB of read-only data).
pub const BUDGET: usize = 32 * 1024;
/// Entries per lookup block.
pub const PER_BLOCK: usize = 32;

/// CMUdict: word → pronunciation variants.
pub type Dict = HashMap<String, Vec<Vec<Phone>>>;

fn run_ok(cmd: &mut Command) -> bool {
    cmd.status().is_ok_and(|s| s.success())
}

fn sha256(path: &Path) -> String {
    Command::new("sha256sum")
        .arg(path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split(' ').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// Makes sure the data is in `cache`: `cmudict.dict` and `books/<n>.txt`.
pub fn fetch(cache: &Path) -> Result<(), String> {
    std::fs::create_dir_all(cache.join("books")).map_err(|e| e.to_string())?;
    let dict = cache.join("cmudict.dict");
    if !dict.exists() || sha256(&dict) != CMUDICT_SHA256 {
        if !run_ok(Command::new("curl").args(["-sSfL", "-o"]).arg(&dict).arg(CMUDICT_URL)) {
            return Err(format!("could not download {CMUDICT_URL}"));
        }
        let got = sha256(&dict);
        if got != CMUDICT_SHA256 {
            return Err(format!("cmudict.dict: SHA-256 {got}, expected {CMUDICT_SHA256}"));
        }
    }
    for id in BOOKS {
        let out = cache.join("books").join(format!("{id}.txt"));
        if out.metadata().is_ok_and(|m| m.len() > 10_000) {
            continue;
        }
        let s = id.to_string();
        let dir = if s.len() == 1 {
            format!("0/{s}")
        } else {
            let mut d = String::new();
            for ch in s[..s.len() - 1].chars() {
                d.push(ch);
                d.push('/');
            }
            d + &s
        };
        let ok = [format!("{s}-0.txt"), format!("{s}.txt"), format!("{s}-8.txt")].iter().any(|f| {
            run_ok(Command::new("curl").args(["-sSfL", "-o"]).arg(&out).arg(format!("{MIRROR}/{dir}/{f}")))
        });
        if !ok {
            let _ = std::fs::remove_file(&out);
            return Err(format!("could not download Gutenberg book {id} from {MIRROR}"));
        }
    }
    Ok(())
}

/// Parses CMUdict text (words of `a`–`z` and `'` only).
pub fn parse_dict(text: &str) -> Dict {
    let mut d: Dict = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let Some(w) = it.next() else { continue };
        let w = w.split('(').next().unwrap_or(w);
        if w.is_empty() || !w.bytes().all(|c| c.is_ascii_lowercase() || c == b'\'') {
            continue;
        }
        let pron: Option<Vec<Phone>> = it.map(|t| Phone::parse(t.as_bytes())).collect();
        if let Some(p) = pron {
            d.entry(w.to_string()).or_default().push(p);
        }
    }
    d
}

/// Counts words (lower case, letters and inner apostrophes) in a text,
/// between Project Gutenberg's start and end markers when present.
pub fn count_words(text: &str, counts: &mut HashMap<String, u64>) {
    let start = text.find("*** START OF").and_then(|i| text[i..].find('\n').map(|j| i + j)).unwrap_or(0);
    let end = text.find("*** END OF").unwrap_or(text.len()).max(start);
    let body = &text[start..end];
    let mut w = String::new();
    let mut flush = |w: &mut String| {
        let t = w.trim_matches('\'');
        if !t.is_empty() && t.len() <= MAX_WORD {
            *counts.entry(t.to_string()).or_default() += 1;
        }
        w.clear();
    };
    for ch in body.chars() {
        let ch = if ch == '\u{2019}' { '\'' } else { ch };
        if ch.is_ascii_alphabetic() || (ch == '\'' && !w.is_empty()) {
            w.push(ch.to_ascii_lowercase());
        } else {
            flush(&mut w);
        }
    }
    flush(&mut w);
}

/// Words of the string literals of Rust sources under `dirs`.
pub fn ui_words(dirs: &[PathBuf]) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack: Vec<PathBuf> = dirs.to_vec();
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let Ok(src) = std::fs::read_to_string(&p) else { continue };
                for lit in src.split('"').skip(1).step_by(2) {
                    for w in lit.split(|c: char| !c.is_ascii_alphabetic() && c != '\'') {
                        let w = w.trim_matches('\'');
                        // Words as a person reads them: not identifiers.
                        if w.len() >= 2
                            && w.len() <= MAX_WORD
                            && !w[1..].bytes().any(|c| c.is_ascii_uppercase())
                        {
                            out.insert(w.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

fn vowel_stress(p: &[Phone]) -> (Vec<u8>, Option<usize>, usize) {
    let ph: Vec<u8> = p.iter().map(|x| x.ph as u8).collect();
    let vowels: Vec<&Phone> = p.iter().filter(|x| x.ph.is_vowel()).collect();
    (ph, vowels.iter().position(|x| x.stress == Stress::Primary), vowels.len())
}

/// Whether `got` is right against the variants (see the module docs).
/// Returns (phonemes right, phonemes and primary stress right).
pub fn judge(got: &[Phone], variants: &[Vec<Phone>]) -> (bool, bool) {
    let (g, gs, gv) = vowel_stress(got);
    let mut ph_ok = false;
    for v in variants {
        let (r, rs, _) = vowel_stress(v);
        if r == g {
            ph_ok = true;
            if gv <= 1 || gs == rs {
                return (true, true);
            }
        }
    }
    (ph_ok, false)
}

/// The primary-stress vowel index of `p`.
pub fn primary_index(p: &[Phone]) -> Option<usize> {
    vowel_stress(p).1
}

/// A lexicon under construction.
pub struct Building<'a>(pub &'a BTreeMap<Vec<u8>, Pron>);

impl Lookup for Building<'_> {
    fn lookup(&self, word: &[u8], out: &mut [Phone]) -> Option<Entry> {
        match self.0.get(word)? {
            Pron::Full(v) => {
                let n = v.len().min(out.len());
                out[..n].copy_from_slice(&v[..n]);
                Some(Entry::Full(n))
            }
            Pron::Stress(s) => Some(Entry::Stress(*s)),
        }
    }
}

/// One evaluated word.
#[derive(Clone, Debug)]
pub struct Item {
    /// The word.
    pub word: String,
    /// Its weight (corpus count, or the forced weight).
    pub weight: u64,
}

/// Accuracy numbers over a word list.
#[derive(Clone, Copy, Debug, Default)]
pub struct Score {
    /// Words.
    pub words: u64,
    /// Words with the phonemes right.
    pub ph_words: u64,
    /// Words fully right (phonemes and primary stress).
    pub ok_words: u64,
    /// Weight (tokens).
    pub tokens: u64,
    /// Tokens with the phonemes right.
    pub ph_tokens: u64,
    /// Tokens fully right.
    pub ok_tokens: u64,
}

impl Score {
    /// Percentage helper.
    pub fn pct(a: u64, b: u64) -> f64 {
        if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
    }

    /// One line of numbers.
    pub fn line(&self) -> String {
        format!(
            "words {:>6}: phonemes {:5.1} %, +stress {:5.1} %; tokens: phonemes {:5.1} %, +stress {:5.1} %",
            self.words,
            Self::pct(self.ph_words, self.words),
            Self::pct(self.ok_words, self.words),
            Self::pct(self.ph_tokens, self.tokens),
            Self::pct(self.ok_tokens, self.tokens)
        )
    }
}

/// Scores `f` over `items`.
pub fn score(items: &[Item], dict: &Dict, mut f: impl FnMut(&str, &mut [Phone]) -> usize) -> Score {
    let mut s = Score::default();
    let mut buf = [Phone::new(crate::phoneme::Ph::AH, Stress::None); MAX_WORD_PHONES];
    for it in items {
        let Some(v) = dict.get(&it.word) else { continue };
        let n = f(&it.word, &mut buf);
        let (ph, ok) = judge(&buf[..n], v);
        s.words += 1;
        s.tokens += it.weight;
        if ph {
            s.ph_words += 1;
            s.ph_tokens += it.weight;
        }
        if ok {
            s.ok_words += 1;
            s.ok_tokens += it.weight;
        }
    }
    s
}

/// The rules alone.
pub fn rules_only(word: &str, out: &mut [Phone]) -> usize {
    by_rules(word.as_bytes(), out)
}

/// The entry `word` needs given the set `set` (`None`: right already).
fn needed(word: &str, variants: &[Vec<Phone>], set: &BTreeMap<Vec<u8>, Pron>) -> Option<Pron> {
    let mut buf = [Phone::new(crate::phoneme::Ph::AH, Stress::None); MAX_WORD_PHONES];
    let (n, _) = pronounce(&Building(set), word.as_bytes(), &mut buf);
    if judge(&buf[..n], variants).1 {
        return None;
    }
    // Rules' phonemes right, stress wrong: a stress-only entry.
    let k = by_rules(word.as_bytes(), &mut buf);
    let (ph, _) = judge(&buf[..k], variants);
    if ph {
        for v in variants {
            let (a, _, _) = vowel_stress(v);
            let (b, _, _) = vowel_stress(&buf[..k]);
            if a == b
                && let Some(p) = primary_index(v)
                && p < 16
            {
                return Some(Pron::Stress(p as u8));
            }
        }
    }
    Some(Pron::Full(variants[0].clone()))
}

/// The result of a run.
pub struct Outcome {
    /// The encoded lexicon.
    pub bin: Vec<u8>,
    /// The report text.
    pub report: String,
    /// The test fixture (`data/eval.tsv`).
    pub eval: String,
}

/// Builds everything from the data in `cache`; `ui_dirs` are scanned for
/// the interface's words.
pub fn run(cache: &Path, ui_dirs: &[PathBuf], budget: usize) -> Result<Outcome, String> {
    let dict_text = std::fs::read_to_string(cache.join("cmudict.dict")).map_err(|e| e.to_string())?;
    let mut dict = parse_dict(&dict_text);
    for (w, p) in EXTRA {
        let pron: Option<Vec<Phone>> = p.split(' ').map(|t| Phone::parse(t.as_bytes())).collect();
        let pron = pron.ok_or_else(|| format!("EXTRA {w}: bad phoneme in {p}"))?;
        dict.entry(w.to_string()).or_insert_with(|| std::vec![pron]);
    }
    let mut counts: HashMap<String, u64> = HashMap::new();
    for id in BOOKS {
        let p = cache.join("books").join(format!("{id}.txt"));
        let t = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        count_words(&String::from_utf8_lossy(&t), &mut counts);
    }
    let total_tokens: u64 = counts.values().sum();
    let in_dict_tokens: u64 = counts.iter().filter(|(w, _)| dict.contains_key(*w)).map(|(_, c)| c).sum();
    // The corpus, most frequent first (ties by word), CMUdict words only.
    let mut corpus: Vec<Item> = counts
        .iter()
        .filter(|(w, _)| dict.contains_key(*w))
        .map(|(w, &c)| Item { word: w.clone(), weight: c })
        .collect();
    corpus.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.word.cmp(&b.word)));
    // Forced vocabularies at the weight of the corpus's 300th word.
    let forced_weight = corpus.get(300).map_or(1, |i| i.weight);
    let ui = ui_words(ui_dirs);
    let mut forced: Vec<String> = crate::normalise::VOCABULARY.iter().map(|s| s.to_string()).collect();
    forced.extend(ui.iter().cloned());
    forced.retain(|w| dict.contains_key(w));
    forced.sort();
    forced.dedup();
    let mut weight: HashMap<String, u64> = corpus.iter().map(|i| (i.word.clone(), i.weight)).collect();
    for w in &forced {
        let e = weight.entry(w.clone()).or_default();
        *e = (*e).max(forced_weight);
    }
    let mut ranked: Vec<Item> = weight.iter().map(|(w, &c)| Item { word: w.clone(), weight: c }).collect();
    ranked.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.word.cmp(&b.word)));

    // Greedy selection in weight order.
    let mut set: BTreeMap<Vec<u8>, Pron> = BTreeMap::new();
    let mut order: Vec<(String, u64)> = Vec::new();
    for it in &ranked {
        let v = &dict[&it.word];
        if let Some(p) = needed(&it.word, v, &set) {
            set.insert(it.word.as_bytes().to_vec(), p);
            order.push((it.word.clone(), it.weight));
        }
    }
    // Largest prefix of the order within the budget (binary search on N).
    let build = |n: usize| -> (BTreeMap<Vec<u8>, Pron>, Vec<u8>) {
        let mut m = BTreeMap::new();
        for (w, _) in &order[..n] {
            m.insert(w.as_bytes().to_vec(), set[w.as_bytes()].clone());
        }
        // Drop entries the others make unnecessary (stems added later).
        let keys: Vec<Vec<u8>> = m.keys().cloned().collect();
        for k in keys {
            let p = m.remove(&k).unwrap_or(Pron::Stress(0));
            let w = String::from_utf8_lossy(&k).into_owned();
            if needed(&w, &dict[&w], &m).is_some() {
                m.insert(k, p);
            }
        }
        // Forced words stay right: pruning may drop a stem a forced word
        // relied on when it was skipped (2026-11-25: "passes" after "pass"
        // was pruned). Re-add any forced word that needs its own entry.
        loop {
            let mut added = false;
            for w in &forced {
                if let Some(p) = needed(w, &dict[w], &m) {
                    m.insert(w.as_bytes().to_vec(), p);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        let bin = encode(&m, PER_BLOCK);
        (m, bin)
    };
    let (mut lo, mut hi) = (0usize, order.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if build(mid).1.len() <= budget { lo = mid } else { hi = mid - 1 }
    }
    let (chosen, bin) = build(lo);
    let lex = Lexicon::new(&bin).ok_or("encoded lexicon does not parse")?;

    // Report.
    let mut r = String::new();
    let full = chosen.values().filter(|p| matches!(p, Pron::Full(_))).count();
    r += &format!(
        "corpus: {} books, {} tokens, {} types; {:.1} % of the tokens are CMUdict words ({} types)\n",
        BOOKS.len(),
        total_tokens,
        counts.len(),
        Score::pct(in_dict_tokens, total_tokens),
        corpus.len()
    );
    r += &format!(
        "forced vocabulary: {} words in the dictionary ({} interface words, {} normaliser words; {} of these \
         pronunciations hand-written, not CMUdict's) at weight {}\n",
        forced.len(),
        ui.len(),
        crate::normalise::VOCABULARY.len(),
        EXTRA.len(),
        forced_weight
    );
    r += &format!(
        "lexicon: {} entries ({} full, {} stress-only) of {} needed, {} bytes (budget {})\n\n",
        chosen.len(),
        full,
        chosen.len() - full,
        order.len(),
        bin.len(),
        budget
    );
    let tiers = [1000usize, 5000, 20000, corpus.len()];
    r += "rules alone (NRL + stress heuristic) against CMUdict, by frequency rank:\n";
    for &t in &tiers {
        let t = t.min(corpus.len());
        r += &format!("  top {:>6}  {}\n", t, score(&corpus[..t], &dict, rules_only).line());
    }
    r += "\nrules + lexicon (as shipped):\n";
    for &t in &tiers {
        let t = t.min(corpus.len());
        let s = score(&corpus[..t], &dict, |w, out| pronounce(&lex, w.as_bytes(), out).0);
        r += &format!("  top {:>6}  {}\n", t, s.line());
    }
    let fs: Vec<Item> = forced.iter().map(|w| Item { word: w.clone(), weight: 1 }).collect();
    let s = score(&fs, &dict, |w, out| pronounce(&lex, w.as_bytes(), out).0);
    r += &format!("  forced      {}\n", s.line());
    r += "\naccuracy/size curve (first N needed entries in weight order, all CMUdict words of the corpus):\n";
    r += "  entries    bytes   tokens right (phonemes+stress)\n";
    let mut n = 0usize;
    let mut steps: Vec<usize> = Vec::new();
    while n < order.len() {
        steps.push(n);
        n = if n < 1000 {
            n + 250
        } else if n < 8000 {
            n + 1000
        } else {
            n * 2
        };
    }
    steps.push(lo);
    steps.push(order.len());
    steps.sort_unstable();
    steps.dedup();
    for n in steps {
        let (_, b) = build(n);
        let l = Lexicon::new(&b).ok_or("curve lexicon does not parse")?;
        let s = score(&corpus, &dict, |w, out| pronounce(&l, w.as_bytes(), out).0);
        let mark = if n == lo { "  <- shipped" } else { "" };
        r += &format!(
            "  {:>7} {:>8}   {:5.1} %  (phonemes {:5.1} %){}\n",
            n,
            b.len(),
            Score::pct(s.ok_tokens, s.tokens),
            Score::pct(s.ph_tokens, s.tokens),
            mark
        );
    }

    // The fixture: the top 1,500 words, every 20th of ranks 1,501–40,000
    // (with that stride, so weighted sums estimate the whole corpus) and
    // the forced vocabulary (stride 0), with all CMUdict variants.
    let mut eval = String::from(
        "# word\tcount\tstride\tCMUdict variants (|); generated by `cargo xtask tts-lexicon` (CMUdict: THIRD-PARTY.md)\n",
    );
    let mut seen = std::collections::BTreeSet::new();
    let mut add = |w: &str, c: u64, stride: u64, eval: &mut String| {
        if seen.insert(w.to_string()) {
            let vs: Vec<String> = dict[w]
                .iter()
                .map(|v| {
                    let mut b = [0u8; 512];
                    let k = crate::phoneme::write_arpabet(v, &mut b);
                    String::from_utf8_lossy(&b[..k]).into_owned()
                })
                .collect();
            *eval += &format!("{w}\t{c}\t{stride}\t{}\n", vs.join("|"));
        }
    };
    for (i, it) in corpus.iter().enumerate().take(40_000) {
        if i < 1500 {
            add(&it.word, it.weight, 1, &mut eval);
        } else if i % 20 == 0 {
            add(&it.word, it.weight, 20, &mut eval);
        }
    }
    for w in &forced {
        add(w, 0, 0, &mut eval);
    }
    Ok(Outcome { bin, report: r, eval })
}
