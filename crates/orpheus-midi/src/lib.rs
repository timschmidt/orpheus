//! `orpheus-midi`: Standard MIDI File playback on `orpheus-synth`
//! (docs/plan/04-audio.md §9; WP MIDI-1).
//!
//! ```ignore
//! use orpheus_midi::{MidiPlayer, Options, SliceSource};
//! let mut src = SliceSource::new(&file);
//! let mut p = MidiPlayer::new(Options::new(48_000));
//! let info = p.open_from(&mut src, false)?;          // header, tracks, scan
//! let mut block = [0i16; 2 * 1152];
//! while p.render_from(&mut src, &mut block)? > 0 { /* to the DAC ring */ }
//! p.seek_from(&mut src, 30 * 48_000)?;               // re-scan to 0:30
//! ```
//!
//! # Layers
//!
//! * [`smf`]: the file: header, track chunks, events; the tracks merged in
//!   time order through [`smf::MAX_TRACKS`] cursors of [`smf::WINDOW`]
//!   bytes each that ask for bytes ([`smf::Pull::Need`]) instead of holding
//!   the file. Adversarial input is expected: no length is trusted.
//! * [`timing`]: ticks to output frames, exactly (tempo changes, SMPTE),
//!   and the tempo map.
//! * [`seq`]: channel messages to engine events (controllers, RPN bend
//!   range, program changes, mute and solo).
//! * [`gm`]: the General MIDI sound set: 32 sounds of our own for the 128
//!   programs, kits for channel 10, written out as an `OSB1` bank.
//! * [`player`]: [`MidiPlayer`], the decoder-like PCM source with open,
//!   render, seek, position and duration, lyrics; [`player::summary`] for
//!   duration, title and lyrics without playing.
//! * [`kar`]: karaoke and lyric text conventions.
//! * [`write`], [`demo`]: writing files; three songs of our own.
//!
//! `no_std`, no allocation, no `unsafe`; `f32` arithmetic only in the
//! synthesis engine's rules (the sequencer computes a handful of gains).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod demo;
pub mod gm;
pub mod kar;
pub mod player;
pub mod seq;
pub mod smf;
pub mod source;
pub mod timing;
pub mod write;

pub use player::{LyricEvent, MidiPlayer, Options, SongInfo, summary};
pub use smf::{Division, MidiError, MidiEvent, Msg, Need, Pull, Smf};
pub use source::{MidiSource, SliceSource};

/// Whether `head` (the first bytes of a file) is a MIDI file (`MThd`, or
/// `RIFF`…`RMID`).
#[must_use]
pub fn probe(head: &[u8]) -> bool {
    head.starts_with(b"MThd") || (head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"RMID")
}

/// Whether a file name has a MIDI extension (`.mid`, `.midi`, `.kar`,
/// `.rmi`); the second value says karaoke.
#[must_use]
pub fn extension(name: &str) -> Option<bool> {
    let (_, ext) = name.rsplit_once('.')?;
    let is = |e: &str| ext.eq_ignore_ascii_case(e);
    if is("kar") {
        Some(true)
    } else if is("mid") || is("midi") || is("rmi") {
        Some(false)
    } else {
        None
    }
}

/// RAM of the player's parts, bytes (`size_of`, this build).
pub mod ram {
    use core::mem::size_of;

    /// The whole player (engine included).
    #[must_use]
    pub fn player() -> usize {
        size_of::<crate::MidiPlayer>()
    }

    /// The parser with its track cursors.
    #[must_use]
    pub fn smf() -> usize {
        size_of::<crate::Smf>()
    }

    /// The channel state.
    #[must_use]
    pub fn sequencer() -> usize {
        size_of::<crate::seq::Sequencer>()
    }

    /// The synthesis engine.
    #[must_use]
    pub fn engine() -> usize {
        size_of::<orpheus_synth::Engine>()
    }
}
