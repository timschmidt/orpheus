//! `orpheus-tts`: the text-to-speech front end (docs/plan/04-audio.md §8;
//! WP TTS-1). English text in UTF-8, in pieces, becomes a stream of
//! phonemes with stress and durations, word marks and pause/intonation
//! marks, ready for the formant synthesiser (TTS-2).
//!
//! ```
//! use orpheus_tts::{Converter, Options, Speed, Token};
//! let mut tts = Converter::new(Options::default(), Speed::NORMAL);
//! let mut phones = 0;
//! tts.convert(b"Track 3 of 12, 4:05.", |_, t| {
//!     if let Token::Phone(_) = t {
//!         phones += 1;
//!     }
//! });
//! assert!(phones > 20);
//! ```
//!
//! For long texts feed pieces as space allows and pull tokens in between
//! ([`Converter::feed`], [`Converter::next_token`], [`Converter::finish`]);
//! the converter's RAM is fixed ([`convert`]).
//!
//! # Parts
//!
//! * [`normalise`]: numbers, dates, times, currency, units, abbreviations,
//!   initials, Roman numerals, file names, URLs, punctuation → words,
//!   letters and breaks.
//! * [`rules`]: the NRL letter-to-sound rules (Elovitz et al. 1976, public
//!   domain), [`stress`]: the stress heuristic, [`lexicon`]: the exception
//!   lexicon from CMUdict, [`words`]: one word to phones.
//! * [`phoneme`]: the ARPAbet phoneme set, [`prosody`]: break kinds and
//!   contours, [`duration`]: the speed setting and the duration model.
//! * `lexgen` (feature `build`): the tool that builds the lexicon
//!   (`cargo xtask tts-lexicon`).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod convert;
pub mod duration;
pub mod fold;
pub mod lexicon;
pub mod normalise;
pub mod number;
pub mod phoneme;
pub mod prosody;
pub mod rules;
pub mod stress;
pub mod words;

#[cfg(feature = "build")]
pub mod lexgen;

pub use convert::{Converter, Pause, Seg, Token, Word};
pub use duration::Speed;
pub use normalise::Options;
pub use phoneme::{Ph, Phone, Stress};
pub use prosody::{BreakKind, Contour};
