//! `orpheus-tts`: text to speech (docs/plan/04-audio.md §8). The front end
//! (WP TTS-1) turns English text in UTF-8, in pieces, into a stream of
//! phonemes with stress and durations, word marks and pause/intonation
//! marks; the formant synthesiser (WP TTS-2) turns that stream into audio.
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
//! Text to audio:
//!
//! ```
//! use orpheus_tts::{Config, Event, Options, Speaker};
//! let mut sp = Speaker::new(Config { rate: 16_000, channels: 1, ..Config::default() }, Options::default());
//! sp.feed(b"Hello there.");
//! sp.finish();
//! let mut block = [0i16; 512];
//! let mut words = 0;
//! let mut frames = 0;
//! loop {
//!     let n = sp.render(&mut block, |_, e| words += usize::from(matches!(e, Event::Word { .. })));
//!     frames += n;
//!     if n < block.len() {
//!         break; // done (or, while text is still being fed, starved)
//!     }
//! }
//! assert_eq!(words, 2);
//! assert!(frames > 8_000);
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
//! * [`speak`]: the streaming renderer ([`Synth`], tokens → audio, with
//!   intonation, events and stop) and [`Speaker`] (text → audio);
//!   [`klatt`]: the cascade/parallel formant synthesiser at 16 kHz;
//!   [`targets`]: the phoneme targets and loci; [`voice`]: the voices;
//!   [`resample`]: 16 kHz to the output rate.
//! * `lexgen` (feature `build`): the tool that builds the lexicon
//!   (`cargo xtask tts-lexicon`).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod convert;
pub mod duration;
pub mod fold;
pub mod klatt;
pub mod lexicon;
pub mod normalise;
pub mod number;
pub mod phoneme;
pub mod prosody;
pub mod resample;
pub mod rules;
pub mod speak;
pub mod stress;
pub mod targets;
pub mod voice;
pub mod words;

#[cfg(feature = "build")]
pub mod lexgen;

pub use convert::{Converter, Pause, Seg, Token, Word};
pub use duration::Speed;
pub use normalise::Options;
pub use phoneme::{Ph, Phone, Stress};
pub use prosody::{BreakKind, Contour};
pub use speak::{Config, Event, Speaker, Stop, Synth};
pub use voice::Voice;
