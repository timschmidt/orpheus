//! English letter-to-sound rules after Elovitz, Johnson, McHugh and Shore,
//! *Automatic Translation of English Text to Phonetics by Means of
//! Letter-to-Sound Rules*, NRL Report 7948, Naval Research Laboratory,
//! January 1976 (a work of the US government, public domain). The table is
//! written out from the report's description in this crate's own notation;
//! no implementation was copied.
//!
//! # Notation
//!
//! One rule per line, `L[M]R=P`: the letters `M` are pronounced `P` (ARPAbet,
//! space-separated, empty for silent letters; `AX` is the reduced vowel, an
//! unstressable `AH`) when the text to their left matches `L` (read from
//! right to left, nearest letter first) and the text to their right matches
//! `R`. Rules are tried in order within the group of `M`'s first letter; the
//! first match wins and the scan moves past `M`. Context symbols:
//!
//! | Symbol | Matches |
//! |---|---|
//! | space | the word boundary |
//! | `#` | one or more vowels (A E I O U) |
//! | `*` | one or more consonants |
//! | `:` | zero or more consonants |
//! | `^` | one consonant |
//! | `.` | one voiced consonant: B D V G J L M N R W Z |
//! | `+` | one front vowel: E I Y |
//! | `%` | one suffix: ER E ES ED ING ELY (right context only) |
//! | `&` | one sibilant: S C G Z X J CH SH |
//! | `@` | one of T S R D L Z N J TH CH SH (a consonant after which U is /uː/) |
//!
//! Y counts as a consonant for these classes, as in the report. Matching
//! of `#`, `*` and `:` is greedy without backtracking.

use crate::phoneme::Ph;

/// The rule table (see the module documentation for the notation).
pub const TABLE: &str = "\
 [A] =AX
 [ARE] =AA R
 [AR]O=AX R
[AR]#=EH R
 ^[AS]#=EY S
[A]WA=AX
[AW]=AO
 :[ANY]=EH N IY
[A]^+#=EY
#:[ALLY]=AX L IY
 [AL]#=AX L
[AGAIN]=AX G EH N
#:[AG]E=IH JH
[A]^+:#=AE
 :[A]^+ =EY
[A]^%=EY
 [ARR]=AX R
[ARR]=AE R
 :[AR] =AA R
[AR] =ER
[AR]=AA R
[AIR]=EH R
[AI]=EY
[AY]=EY
[AU]=AO
#:[AL] =AX L
#:[ALS] =AX L Z
[ALK]=AO K
[AL]^=AO L
 :[ABLE]=EY B AX L
[ABLE]=AX B AX L
[ANG]+=EY N JH
[A]=AE
 [BE]^#=B IH
[BEING]=B IY IH NG
 [BOTH] =B OW TH
 [BUS]#=B IH Z
[BUIL]=B IH L
[B]=B
 [CH]^=K
^E[CH]=K
[CH]=CH
 S[CI]#=S AY
[CI]A=SH
[CI]O=SH
[CI]EN=SH
[C]+=S
[CK]=K
[COM]%=K AH M
[C]=K
#:[DED] =D IH D
.E[D] =D
#:^E[D] =T
 [DE]^#=D IH
 [DO] =D UW
 [DOES]=D AH Z
 [DOING]=D UW IH NG
 [DOW]=D AW
[DU]A=JH UW
[D]=D
#:[E] =
 :[E] =IY
#[ED] =D
#:[E]D =
[EV]ER=EH V
[E]^%=IY
[ERI]#=IY R IY
[ERI]=EH R IH
#:[ER]#=ER
[ER]#=EH R
[ER]=ER
 [EVEN]=IY V EH N
#:[E]W=
@[EW]=UW
[EW]=Y UW
[E]O=IY
#:&[ES] =IH Z
#:[E]S =
#:[ELY] =L IY
#:[EMENT]=M EH N T
[EFUL]=F UH L
[EE]=IY
[EARN]=ER N
 [EAR]^=ER
[EAD]=EH D
#:[EA] =IY AX
[EA]SU=EH
[EA]=IY
[EIGH]=EY
[EI]=IY
 [EYE]=AY
[EY]=IY
[EU]=Y UW
[E]=EH
[FUL]=F UH L
[F]=F
[GIV]=G IH V
 [G]I^=G
[GE]T=G EH
SU[GGES]=G JH EH S
[GG]=G
 B#[G]=G
[G]+=JH
[GREAT]=G R EY T
#[GH]=
[G]=G
 [HAV]=HH AE V
 [HERE]=HH IY R
 [HOUR]=AW ER
[HOW]=HH AW
[H]#=HH
[H]=
 [IN]=IH N
 [I] =AY
[IN]D=AY N
[IER]=IY ER
#:R[IED] =IY D
[IED] =AY D
[IEN]=IY EH N
[IE]T=AY EH
 :[I]%=AY
[I]%=IY
[IE]=IY
[I]^+:#=IH
[IR]#=AY R
[IZ]%=AY Z
[IS]%=AY Z
[I]D%=AY
+^[I]^+=IH
[I]T%=AY
#:^[I]^+=IH
[I]^+=AY
[IR]=ER
[IGH]=AY
[ILD]=AY L D
[IGN] =AY N
[IGN]^=AY N
[IGN]%=AY N
[IQUE]=IY K
[I]=IH
[J]=JH
 [K]N=
[K]=K
[LO]C#=L OW
L[L]=
#:^[L]%=AX L
[LEAD]=L IY D
[L]=L
[MOV]=M UW V
[M]=M
E[NG]+=N JH
[NG]R=NG G
[NG]#=NG G
[NGL]%=NG G AX L
[NG]=NG
[NK]=NG K
 [NOW] =N AW
[N]=N
[OF] =AX V
[OROUGH]=ER OW
#:[OR] =ER
#:[ORS] =ER Z
[OR]=AO R
 [ONE]=W AH N
[OW]=OW
 [OVER]=OW V ER
[OV]=AH V
[O]^%=OW
[O]^EN=OW
[O]^I#=OW
[OL]D=OW L
[OUGHT]=AO T
[OUGH]=AH F
 [OU]=AW
H[OU]S#=AW
[OUS]=AX S
[OUR]=AO R
[OULD]=UH D
^[OU]^L=AH
[OUP]=UW P
[OU]=AW
[OY]=OY
[OING]=OW IH NG
[OI]=OY
[OOR]=AO R
[OOK]=UH K
[OOD]=UH D
[OO]=UW
[O]E=OW
[O] =OW
[OA]=OW
 [ONLY]=OW N L IY
 [ONCE]=W AH N S
C[O]N=AA
[O]NG=AO
 :^[O]N=AH
I[ON]=AX N
#:[ON] =AX N
#^[ON]=AX N
[O]ST =OW
[OF]^=AO F
[OTHER]=AH DH ER
[OSS] =AO S
#:^[OM]=AH M
[O]=AA
[PH]=F
[PEOP]=P IY P
[POW]=P AW
[PUT] =P UH T
[P]=P
[QUAR]=K W AO R
[QU]=K W
[Q]=K
 [RE]^#=R IY
[R]=R
[SH]=SH
#[SION]=ZH AX N
[SOME]=S AH M
#[SUR]#=ZH ER
[SUR]#=SH ER
#[SU]#=ZH UW
#[SSU]#=SH UW
#[SED] =Z D
#[S]#=Z
[SAID]=S EH D
^[SION]=SH AX N
[S]S=
.[S] =Z
#:.E[S] =Z
#:^##[S] =Z
#:^#[S] =S
U[S] =S
 :#[S] =Z
 [SCH]=S K
[S]C+=
#[SM]=Z M
[S]=S
 [THE] =DH AX
 [TO] =T UW
 [THAT] =DH AE T
 [THIS] =DH IH S
 [THEY]=DH EY
 [THERE]=DH EH R
[THER]=DH ER
[THEIR]=DH EH R
 [THAN] =DH AE N
 [THEM] =DH EH M
[THESE] =DH IY Z
 [THEN]=DH EH N
[THROUGH]=TH R UW
[THOSE]=DH OW Z
[THOUGH] =DH OW
 [THUS]=DH AH S
[TH]=TH
#:[TED] =T IH D
S[TI]#N=CH
[TI]O=SH
[TI]A=SH
[TIEN]=SH AX N
[TUR]#=CH ER
[TU]A=CH UW
 [TWO]=T UW
[T]=T
 [UN]I=Y UW N
 [UN]=AH N
 [UPON]=AX P AO N
@[UR]#=UH R
[UR]#=Y UH R
[UR]=ER
[U]^ =AH
[U]^^=AH
[UY]=AY
 G[U]#=
G[U]%=
G[U]#=W
#N[U]=Y UW
@[U]=UW
[U]=Y UW
[VIEW]=V Y UW
[V]=V
 [WERE]=W ER
[WA]S=W AA
[WA]T=W AA
[WHERE]=W EH R
[WHAT]=W AA T
[WHOL]=HH OW L
[WHO]=HH UW
[WH]=W
[WAR]=W AO R
[WOR]^=W ER
[WR]=R
[W]=W
[X]=K S
[YOUNG]=Y AH NG
 [YOU]=Y UW
 [YES]=Y EH S
 [Y]=Y
#:^[Y] =IY
#:^[Y]I=IY
 :[Y] =AY
 :[Y]#=AY
 :[Y]^+:#=IH
 :[Y]^#=AY
[Y]=IH
[Z]=Z
";

/// Number of rules in [`TABLE`].
pub const fn rule_count() -> usize {
    let b = TABLE.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            n += 1;
        }
        i += 1;
    }
    n
}

/// Byte offset of the first rule of each letter's group in [`TABLE`]
/// (`A`..`Z`, then the end), computed at compile time.
const GROUPS: [u16; 27] = groups();

const fn groups() -> [u16; 27] {
    let b = TABLE.as_bytes();
    let mut g = [u16::MAX; 27];
    let mut line = 0usize;
    while line < b.len() {
        // The group letter: the first letter after '['.
        let mut j = line;
        while b[j] != b'[' {
            j += 1;
        }
        let letter = (b[j + 1] - b'A') as usize;
        if g[letter] == u16::MAX {
            g[letter] = line as u16;
        }
        while b[line] != b'\n' {
            line += 1;
        }
        line += 1;
    }
    g[26] = b.len() as u16;
    // Groups are contiguous and in alphabetical order; fill gaps (none
    // expected) with the next group's start.
    let mut k = 25;
    loop {
        if g[k] == u16::MAX {
            g[k] = g[k + 1];
        }
        if k == 0 {
            break;
        }
        k -= 1;
    }
    g
}

/// One phoneme produced by the rules: `reduced` marks `AX`, a vowel that
/// never takes stress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RulePhone {
    /// The phoneme.
    pub ph: Ph,
    /// Written `AX` in the table (reduced, unstressable).
    pub reduced: bool,
}

fn is_vowel(c: u8) -> bool {
    matches!(c, b'A' | b'E' | b'I' | b'O' | b'U')
}

fn is_consonant(c: u8) -> bool {
    c.is_ascii_uppercase() && !is_vowel(c)
}

fn is_voiced(c: u8) -> bool {
    matches!(c, b'B' | b'D' | b'V' | b'G' | b'J' | b'L' | b'M' | b'N' | b'R' | b'W' | b'Z')
}

fn is_front(c: u8) -> bool {
    matches!(c, b'E' | b'I' | b'Y')
}

/// The word being translated, padded conceptually with boundaries: index
/// `-1` and `len` are the boundary (byte 0).
struct Word<'a>(&'a [u8]);

impl Word<'_> {
    fn at(&self, i: isize) -> u8 {
        if i < 0 { 0 } else { self.0.get(i as usize).copied().unwrap_or(0) }
    }
}

/// Matches the left context `ctx` ending just before position `pos`.
fn left_matches(w: &Word, ctx: &[u8], pos: isize) -> bool {
    let mut p = pos - 1; // the next letter to look at, moving left
    for &s in ctx.iter().rev() {
        match s {
            b' ' => {
                if w.at(p).is_ascii_uppercase() {
                    return false;
                }
                p -= 1;
            }
            b'#' => {
                if !is_vowel(w.at(p)) {
                    return false;
                }
                while is_vowel(w.at(p)) {
                    p -= 1;
                }
            }
            b':' => {
                while is_consonant(w.at(p)) {
                    p -= 1;
                }
            }
            b'*' => {
                if !is_consonant(w.at(p)) {
                    return false;
                }
                while is_consonant(w.at(p)) {
                    p -= 1;
                }
            }
            b'^' => {
                if !is_consonant(w.at(p)) {
                    return false;
                }
                p -= 1;
            }
            b'.' => {
                if !is_voiced(w.at(p)) {
                    return false;
                }
                p -= 1;
            }
            b'+' => {
                if !is_front(w.at(p)) {
                    return false;
                }
                p -= 1;
            }
            b'&' => {
                let c = w.at(p);
                if c == b'H' && matches!(w.at(p - 1), b'C' | b'S') {
                    p -= 2;
                } else if matches!(c, b'S' | b'C' | b'G' | b'Z' | b'X' | b'J') {
                    p -= 1;
                } else {
                    return false;
                }
            }
            b'@' => {
                let c = w.at(p);
                if c == b'H' && matches!(w.at(p - 1), b'T' | b'C' | b'S') {
                    p -= 2;
                } else if matches!(c, b'T' | b'S' | b'R' | b'D' | b'L' | b'Z' | b'N' | b'J') {
                    p -= 1;
                } else {
                    return false;
                }
            }
            c => {
                if w.at(p) != c {
                    return false;
                }
                p -= 1;
            }
        }
    }
    true
}

/// Matches the right context `ctx` starting at position `pos`.
fn right_matches(w: &Word, ctx: &[u8], pos: isize) -> bool {
    let mut p = pos;
    for &s in ctx {
        match s {
            b' ' => {
                if w.at(p).is_ascii_uppercase() {
                    return false;
                }
                p += 1;
            }
            b'#' => {
                if !is_vowel(w.at(p)) {
                    return false;
                }
                while is_vowel(w.at(p)) {
                    p += 1;
                }
            }
            b':' => {
                while is_consonant(w.at(p)) {
                    p += 1;
                }
            }
            b'*' => {
                if !is_consonant(w.at(p)) {
                    return false;
                }
                while is_consonant(w.at(p)) {
                    p += 1;
                }
            }
            b'^' => {
                if !is_consonant(w.at(p)) {
                    return false;
                }
                p += 1;
            }
            b'.' => {
                if !is_voiced(w.at(p)) {
                    return false;
                }
                p += 1;
            }
            b'+' => {
                if !is_front(w.at(p)) {
                    return false;
                }
                p += 1;
            }
            b'%' => {
                let tail = |s: &[u8]| s.iter().enumerate().all(|(k, &c)| w.at(p + k as isize) == c);
                let mut hit = false;
                for suf in [&b"ING"[..], b"ELY", b"ER", b"ES", b"ED", b"E"] {
                    if tail(suf) {
                        p += suf.len() as isize;
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    return false;
                }
            }
            b'&' => {
                let c = w.at(p);
                if matches!(c, b'C' | b'S') && w.at(p + 1) == b'H' {
                    p += 2;
                } else if matches!(c, b'S' | b'C' | b'G' | b'Z' | b'X' | b'J') {
                    p += 1;
                } else {
                    return false;
                }
            }
            b'@' => {
                let c = w.at(p);
                if matches!(c, b'T' | b'C' | b'S') && w.at(p + 1) == b'H' {
                    p += 2;
                } else if matches!(c, b'T' | b'S' | b'R' | b'D' | b'L' | b'Z' | b'N' | b'J') {
                    p += 1;
                } else {
                    return false;
                }
            }
            c => {
                if w.at(p) != c {
                    return false;
                }
                p += 1;
            }
        }
    }
    true
}

/// Splits one rule line into (left, match, right, phonemes).
fn split(line: &[u8]) -> (&[u8], &[u8], &[u8], &[u8]) {
    let open = line.iter().position(|&c| c == b'[').unwrap_or(0);
    let close = line.iter().position(|&c| c == b']').unwrap_or(open);
    let eq = line.iter().position(|&c| c == b'=').unwrap_or(line.len());
    (
        &line[..open],
        &line[open + 1..close],
        &line[close + 1..eq.max(close + 1)],
        line.get(eq + 1..).unwrap_or(&[]),
    )
}

/// Translates `word` (upper-case ASCII letters; any other byte acts as a
/// boundary) into phonemes by the rules, calling `out` for each one in
/// order. Returns the number of phonemes produced.
pub fn translate(word: &[u8], mut out: impl FnMut(RulePhone)) -> usize {
    let w = Word(word);
    let table = TABLE.as_bytes();
    let mut n = 0;
    let mut i = 0usize;
    while i < word.len() {
        let c = word[i];
        if !c.is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let g = (c - b'A') as usize;
        let (start, end) = (GROUPS[g] as usize, GROUPS[g + 1] as usize);
        let mut advanced = false;
        for line in table[start..end].split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let (l, m, r, p) = split(line);
            if !word[i..].starts_with(m) {
                continue;
            }
            if !left_matches(&w, l, i as isize) || !right_matches(&w, r, (i + m.len()) as isize) {
                continue;
            }
            for tok in p.split(|&b| b == b' ').filter(|t| !t.is_empty()) {
                let (ph, reduced) = if tok == b"AX" { (Some(Ph::AH), true) } else { (Ph::parse(tok), false) };
                if let Some(ph) = ph {
                    out(RulePhone { ph, reduced });
                    n += 1;
                }
            }
            i += m.len().max(1);
            advanced = true;
            break;
        }
        if !advanced {
            i += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    fn tr(w: &str) -> String {
        let mut v = Vec::new();
        translate(w.as_bytes(), |p| v.push(if p.reduced { "AX" } else { p.ph.name() }));
        v.join(" ")
    }

    #[test]
    fn table_is_well_formed() {
        let mut last = b'A';
        for line in TABLE.lines() {
            let b = line.as_bytes();
            let (l, m, r, p) = split(b);
            assert!(!m.is_empty(), "{line}");
            assert!(m[0] >= last, "groups out of order at {line}");
            last = m[0];
            assert!(m.iter().all(|c| c.is_ascii_uppercase()), "{line}");
            for c in l.iter().chain(r) {
                assert!(c.is_ascii_uppercase() || b" #*:^.+%&@".contains(c), "{line}");
            }
            assert!(!l.contains(&b'%'), "% only on the right: {line}");
            for t in p.split(|&c| c == b' ').filter(|t| !t.is_empty()) {
                assert!(t == b"AX" || Ph::parse(t).is_some(), "{line}");
            }
        }
        // Each group ends with its catch-all rule.
        for g in 0..26u8 {
            let c = b'A' + g;
            let last_rule = TABLE
                .lines()
                .rfind(|l| split(l.as_bytes()).1.first() == Some(&c))
                .unwrap_or_else(|| panic!("no group {}", c as char));
            let (l, m, r, _) = split(last_rule.as_bytes());
            assert!(l.is_empty() && m == [c] && r.is_empty(), "group {} ends with {last_rule}", c as char);
        }
        assert!(rule_count() > 300);
    }

    #[test]
    fn examples() {
        assert_eq!(tr("THE"), "DH AX");
        assert_eq!(tr("CAT"), "K AE T");
        assert_eq!(tr("MAKE"), "M EY K");
        assert_eq!(tr("NATION"), "N EY SH AX N");
        assert_eq!(tr("SHIP"), "SH IH P");
        assert_eq!(tr("PHONE"), "F OW N");
        assert_eq!(tr("BOOK"), "B UH K");
        assert_eq!(tr("RIGHT"), "R AY T");
        assert_eq!(tr("DOGS"), "D AA G Z");
        assert_eq!(tr("WALKED"), "W AO K T");
    }

    #[test]
    fn non_letters_are_boundaries() {
        assert_eq!(tr("A-B"), tr("A") + " " + &tr("B"));
        assert_eq!(tr(""), "");
    }
}
