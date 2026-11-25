//! The phoneme set: the 39 phonemes of ARPAbet as CMUdict uses them, with
//! lexical stress on vowels, and the classes the duration model and the
//! synthesiser (TTS-2) work with.

/// One English phoneme (ARPAbet, CMUdict's set). The discriminants are
/// stable: the lexicon stores them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Ph {
    /// `father`
    AA,
    /// `cat`
    AE,
    /// `but`; unstressed: the schwa of `about`
    AH,
    /// `caught`
    AO,
    /// `cow`
    AW,
    /// `hide`
    AY,
    /// `red`
    EH,
    /// `bird`; unstressed: `butter`
    ER,
    /// `say`
    EY,
    /// `bit`
    IH,
    /// `see`
    IY,
    /// `go`
    OW,
    /// `toy`
    OY,
    /// `book`
    UH,
    /// `two`
    UW,
    /// `bee`
    B,
    /// `cheese`
    CH,
    /// `dee`
    D,
    /// `thee`
    DH,
    /// `fee`
    F,
    /// `green`
    G,
    /// `he`
    HH,
    /// `gee`
    JH,
    /// `key`
    K,
    /// `lee`
    L,
    /// `me`
    M,
    /// `knee`
    N,
    /// `ping`
    NG,
    /// `pee`
    P,
    /// `read`
    R,
    /// `sea`
    S,
    /// `she`
    SH,
    /// `tea`
    T,
    /// `theta`
    TH,
    /// `vee`
    V,
    /// `we`
    W,
    /// `yield`
    Y,
    /// `zee`
    Z,
    /// `seizure`
    ZH,
}

/// Number of phonemes.
pub const PHONEMES: usize = 39;

/// Every phoneme in discriminant order.
pub const ALL: [Ph; PHONEMES] = [
    Ph::AA,
    Ph::AE,
    Ph::AH,
    Ph::AO,
    Ph::AW,
    Ph::AY,
    Ph::EH,
    Ph::ER,
    Ph::EY,
    Ph::IH,
    Ph::IY,
    Ph::OW,
    Ph::OY,
    Ph::UH,
    Ph::UW,
    Ph::B,
    Ph::CH,
    Ph::D,
    Ph::DH,
    Ph::F,
    Ph::G,
    Ph::HH,
    Ph::JH,
    Ph::K,
    Ph::L,
    Ph::M,
    Ph::N,
    Ph::NG,
    Ph::P,
    Ph::R,
    Ph::S,
    Ph::SH,
    Ph::T,
    Ph::TH,
    Ph::V,
    Ph::W,
    Ph::Y,
    Ph::Z,
    Ph::ZH,
];

const NAMES: [&str; PHONEMES] = [
    "AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW", "OY", "UH", "UW", "B", "CH", "D",
    "DH", "F", "G", "HH", "JH", "K", "L", "M", "N", "NG", "P", "R", "S", "SH", "T", "TH", "V", "W", "Y", "Z",
    "ZH",
];

/// The broad classes the duration model and the synthesiser distinguish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Monophthong.
    Vowel,
    /// AW, AY, EY, OW, OY: a moving vowel.
    Diphthong,
    /// B D G P T K.
    Stop,
    /// F V TH DH S Z SH ZH.
    Fricative,
    /// CH JH.
    Affricate,
    /// M N NG.
    Nasal,
    /// L R.
    Liquid,
    /// W Y.
    Glide,
    /// HH.
    Aspirate,
}

impl Ph {
    /// From the discriminant; `None` above 38.
    pub const fn from_u8(v: u8) -> Option<Ph> {
        if (v as usize) < PHONEMES { Some(ALL[v as usize]) } else { None }
    }

    /// The ARPAbet name (`"AA"`, `"NG"`).
    pub const fn name(self) -> &'static str {
        NAMES[self as usize]
    }

    /// From an ARPAbet name without a stress digit (upper case).
    pub fn parse(s: &[u8]) -> Option<Ph> {
        let mut i = 0;
        while i < PHONEMES {
            if NAMES[i].as_bytes() == s {
                return Some(ALL[i]);
            }
            i += 1;
        }
        None
    }

    /// Whether this is a vowel (carries stress).
    pub const fn is_vowel(self) -> bool {
        (self as u8) <= Ph::UW as u8
    }

    /// The phoneme's class.
    pub const fn class(self) -> Class {
        use Ph::*;
        match self {
            AW | AY | EY | OW | OY => Class::Diphthong,
            AA | AE | AH | AO | EH | ER | IH | IY | UH | UW => Class::Vowel,
            B | D | G | P | T | K => Class::Stop,
            F | V | TH | DH | S | Z | SH | ZH => Class::Fricative,
            CH | JH => Class::Affricate,
            M | N | NG => Class::Nasal,
            L | R => Class::Liquid,
            W | Y => Class::Glide,
            HH => Class::Aspirate,
        }
    }

    /// Whether the vocal folds vibrate (vowels, sonorants, voiced obstruents).
    pub const fn is_voiced(self) -> bool {
        !matches!(self, Ph::P | Ph::T | Ph::K | Ph::F | Ph::TH | Ph::S | Ph::SH | Ph::CH | Ph::HH)
    }

    /// Whether this is a consonant that can close a syllable as a sonorant
    /// (used by the syllable count of the stress heuristic).
    pub const fn is_sonorant(self) -> bool {
        matches!(self.class(), Class::Nasal | Class::Liquid | Class::Glide) || self.is_vowel()
    }
}

/// Lexical stress of a vowel. Consonants carry [`Stress::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Stress {
    /// Unstressed (CMUdict `0`; `AH` unstressed is the schwa).
    Unstressed = 0,
    /// Primary (CMUdict `1`).
    Primary = 1,
    /// Secondary (CMUdict `2`).
    Secondary = 2,
    /// Not a vowel.
    None = 3,
}

impl Stress {
    /// From the CMUdict digit.
    pub const fn from_digit(d: u8) -> Stress {
        match d {
            b'1' => Stress::Primary,
            b'2' => Stress::Secondary,
            _ => Stress::Unstressed,
        }
    }
}

/// A phoneme with its stress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Phone {
    /// The phoneme.
    pub ph: Ph,
    /// Its stress ([`Stress::None`] for consonants).
    pub stress: Stress,
}

impl Phone {
    /// A consonant, or a vowel with the given stress.
    pub const fn new(ph: Ph, stress: Stress) -> Phone {
        Phone { ph, stress: if ph.is_vowel() { stress } else { Stress::None } }
    }

    /// Packed as one byte: phoneme in the low 6 bits, stress in the top 2
    /// (consonants 0). The lexicon's symbol alphabet.
    pub const fn to_u8(self) -> u8 {
        let s = match self.stress {
            Stress::None => 0,
            s => s as u8,
        };
        (self.ph as u8) | (s << 6)
    }

    /// Inverse of [`Phone::to_u8`].
    pub const fn from_u8(b: u8) -> Option<Phone> {
        match Ph::from_u8(b & 63) {
            Some(ph) => Some(Phone::new(ph, Stress::from_digit(b'0' + (b >> 6)))),
            None => None,
        }
    }

    /// Parses one CMUdict token (`AH0`, `K`).
    pub fn parse(tok: &[u8]) -> Option<Phone> {
        let (name, stress) = match tok.last() {
            Some(d @ b'0'..=b'2') => (&tok[..tok.len() - 1], Stress::from_digit(*d)),
            _ => (tok, Stress::Unstressed),
        };
        Ph::parse(name).map(|ph| Phone::new(ph, stress))
    }
}

/// Writes `phones` as CMUdict text (`HH AH0 L OW1`) into `out`; returns the
/// length written (truncated at the end of `out`).
pub fn write_arpabet(phones: &[Phone], out: &mut [u8]) -> usize {
    let mut n = 0;
    let mut put = |b: u8, n: &mut usize| {
        if *n < out.len() {
            out[*n] = b;
            *n += 1;
        }
    };
    for (i, p) in phones.iter().enumerate() {
        if i > 0 {
            put(b' ', &mut n);
        }
        for &b in p.ph.name().as_bytes() {
            put(b, &mut n);
        }
        if p.stress != Stress::None {
            put(b'0' + p.stress as u8, &mut n);
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for p in ALL {
            assert_eq!(Ph::parse(p.name().as_bytes()), Some(p));
            assert_eq!(Ph::from_u8(p as u8), Some(p));
        }
        assert_eq!(Ph::from_u8(39), None);
        assert_eq!(Ph::parse(b"XX"), None);
    }

    #[test]
    fn packing_round_trips() {
        for p in ALL {
            for s in [Stress::Unstressed, Stress::Primary, Stress::Secondary] {
                let ph = Phone::new(p, s);
                assert_eq!(Phone::from_u8(ph.to_u8()), Some(ph));
            }
        }
        assert_eq!(Phone::parse(b"AH0"), Some(Phone::new(Ph::AH, Stress::Unstressed)));
        assert_eq!(Phone::parse(b"NG"), Some(Phone::new(Ph::NG, Stress::None)));
        let mut buf = [0u8; 32];
        let ph = [Phone::parse(b"HH").unwrap(), Phone::parse(b"AH0").unwrap(), Phone::parse(b"OW1").unwrap()];
        let n = write_arpabet(&ph, &mut buf);
        assert_eq!(&buf[..n], b"HH AH0 OW1");
    }

    #[test]
    fn classes() {
        assert_eq!(Ph::AY.class(), Class::Diphthong);
        assert_eq!(Ph::T.class(), Class::Stop);
        assert!(Ph::UW.is_vowel() && !Ph::B.is_vowel());
        assert!(!Ph::S.is_voiced() && Ph::Z.is_voiced());
    }
}
