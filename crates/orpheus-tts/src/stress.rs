//! Primary stress for words the lexicon does not know: a heuristic over the
//! spelling and the rule phonemes. The lexicon carries CMUdict's stress for
//! the words it holds; everything else gets one primary stress here (no
//! secondary stress), every other vowel unstressed.
//!
//! The heuristic, in order:
//! 1. Neutral endings (`-s`, `-es`, `-ed`, `-ing`, `-ly`, `-ness`, `-ment`,
//!    `-ful`, `-less`, `-er`, `-ers`, `-able`) are set aside; they never take
//!    the stress.
//! 2. Stress-taking endings (`-ee`, `-eer`, `-ese`, `-ette`, `-esque`,
//!    `-oon`, `-ique`, `-aire`) take it on their vowel.
//! 3. Endings that put the stress on the syllable just before them
//!    (`-tion`, `-sion`, `-cian`, `-ic`, `-ical`, `-ial`, `-ian`, `-ious`,
//!    `-eous`, `-ient`, `-ual`, `-ity`, `-ify`, `-logy`, `-graphy`,
//!    `-nomy`, `-meter`, `-ular`).
//! 4. A word of two or more syllables starting with a prefix that is
//!    normally unstressed (`a-`, `be-`, `de-`, `re-`, `con-`, `com-`,
//!    `dis-`, `ex-`, `mis-`, `pre-`, `pro-`, `per-`, `sub-`, `sur-`, `ob-`)
//!    stresses its second syllable.
//! 5. Otherwise the first syllable.
//!
//! Reduced vowels (the rules' `AX`) are never stressed: the stress moves to
//! the next full vowel (or the previous one at the end of the word).
//! Accuracy against CMUdict is measured by the lexicon tool and the
//! `accuracy` test (04 §8 "as built").

use crate::phoneme::{Phone, Stress};
use crate::rules::RulePhone;

const NEUTRAL: [(&[u8], u8); 13] = [
    (b"NESS", 1),
    (b"MENT", 1),
    (b"LESS", 1),
    (b"ABLE", 2),
    (b"FUL", 1),
    (b"ING", 1),
    (b"ERS", 1),
    (b"ER", 1),
    (b"ED", 0),
    (b"LY", 1),
    (b"ES", 0),
    (b"S", 0),
    (b"'S", 0),
];

const TAKING: [&[u8]; 8] = [b"EE", b"EER", b"ESE", b"ETTE", b"ESQUE", b"OON", b"IQUE", b"AIRE"];

const BEFORE: [&[u8]; 19] = [
    b"TION", b"SION", b"CIAN", b"ICAL", b"IC", b"IAL", b"IAN", b"IOUS", b"EOUS", b"IENT", b"UAL", b"ITY",
    b"IFY", b"LOGY", b"GRAPHY", b"NOMY", b"METER", b"ULAR", b"ICS",
];

const PREFIX: [&[u8]; 15] = [
    b"CON", b"COM", b"DIS", b"MIS", b"PRE", b"PRO", b"PER", b"SUB", b"SUR", b"BE", b"DE", b"RE", b"EX",
    b"OB", b"A",
];

/// Counts written vowel groups (A E I O U Y runs) in `s`, a rough syllable
/// count of a spelling fragment.
fn vowel_groups(s: &[u8]) -> usize {
    let mut n = 0;
    let mut prev = false;
    for &c in s {
        let v = matches!(c, b'A' | b'E' | b'I' | b'O' | b'U' | b'Y');
        if v && !prev {
            n += 1;
        }
        prev = v;
    }
    n
}

/// Assigns stress to the rule output `ph` of the upper-case `word`, writing
/// the stressed phones into `out`; returns how many were written.
pub fn assign(word: &[u8], ph: &[RulePhone], out: &mut [Phone]) -> usize {
    let n = ph.len().min(out.len());
    let vowels = ph[..n].iter().filter(|p| p.ph.is_vowel()).count();
    let target = if vowels <= 1 { 0 } else { choose(word, &ph[..n], vowels) };
    let mut v = 0;
    let mut placed = false;
    // The full vowels, to move a stress off a reduced one.
    let full: usize = ph[..n].iter().filter(|p| p.ph.is_vowel() && !p.reduced).count();
    let target = adjust(&ph[..n], target, full);
    for (k, p) in ph[..n].iter().enumerate() {
        let stress = if p.ph.is_vowel() {
            let s = if Some(v) == target && !placed {
                placed = true;
                Stress::Primary
            } else {
                Stress::Unstressed
            };
            v += 1;
            s
        } else {
            Stress::None
        };
        out[k] = Phone::new(p.ph, stress);
    }
    n
}

/// Moves the target vowel index off a reduced vowel; `None` if the word
/// has only reduced vowels and more than one syllable is not involved.
fn adjust(ph: &[RulePhone], target: usize, full: usize) -> Option<usize> {
    let vow: [Option<bool>; 32] = {
        let mut a = [None; 32];
        let mut i = 0;
        for p in ph {
            if p.ph.is_vowel() && i < 32 {
                a[i] = Some(p.reduced);
                i += 1;
            }
        }
        a
    };
    let count = vow.iter().filter(|x| x.is_some()).count();
    if count == 0 {
        return None;
    }
    if full == 0 {
        // Only reduced vowels: a monosyllable is a weak form (unstressed);
        // a longer word stresses its first vowel anyway.
        return if count == 1 { None } else { Some(0) };
    }
    let t = target.min(count - 1);
    if vow[t] == Some(false) {
        return Some(t);
    }
    if let Some(k) = (t + 1..count).find(|&k| vow[k] == Some(false)) {
        return Some(k);
    }
    (0..t).rev().find(|&k| vow[k] == Some(false))
}

fn choose(word: &[u8], ph: &[RulePhone], vowels: usize) -> usize {
    // Vowel indices of the phoneme string that belong to the stem.
    let mut stem = word;
    let mut stem_vowels = vowels;
    for (suf, v) in NEUTRAL {
        if stem.len() > suf.len() + 2 && stem.ends_with(suf) {
            let rest = &stem[..stem.len() - suf.len()];
            if vowel_groups(rest) >= 1 {
                stem = rest;
                stem_vowels = stem_vowels.saturating_sub(v as usize).max(1);
                break;
            }
        }
    }
    let _ = ph;
    for suf in TAKING {
        if stem.len() > suf.len() + 1 && stem.ends_with(suf) {
            return stem_vowels - 1;
        }
    }
    for suf in BEFORE {
        if stem.len() > suf.len() && stem.ends_with(suf) {
            let sv = vowel_groups(suf).max(1);
            return stem_vowels.saturating_sub(sv + 1);
        }
    }
    if stem_vowels >= 2 {
        for pre in PREFIX {
            if stem.len() > pre.len() + 2 && stem.starts_with(pre) {
                // The prefix must be a syllable of its own: followed by a
                // consonant (or, for A-, by one consonant and a vowel).
                let next = stem[pre.len()];
                let cons = next.is_ascii_uppercase() && !b"AEIOU".contains(&next);
                if cons
                    && (pre != b"A" || !b"AEIOUY".contains(&stem[pre.len() + 1]))
                    && vowel_groups(pre) == 1
                {
                    return 1;
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::rules::translate;
    use std::vec::Vec;

    fn primary(word: &str) -> Option<usize> {
        let mut rp = Vec::new();
        translate(word.as_bytes(), |p| rp.push(p));
        let mut out = [Phone::new(crate::phoneme::Ph::AH, Stress::None); 48];
        let n = assign(word.as_bytes(), &rp, &mut out);
        out[..n].iter().filter(|p| p.ph.is_vowel()).position(|p| p.stress == Stress::Primary)
    }

    #[test]
    fn heuristics() {
        assert_eq!(primary("WATER"), Some(0));
        assert_eq!(primary("NATION"), Some(0));
        assert_eq!(primary("INFORMATION"), Some(2));
        assert_eq!(primary("DESCRIBE"), Some(1));
        assert_eq!(primary("CAT"), Some(0));
        assert_eq!(primary("THE"), None);
    }
}
