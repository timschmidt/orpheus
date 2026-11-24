//! Recording without hardware (WP AU-13, 04 §5.2, 05 §7.5).
//!
//! ```text
//! ADC ─DMA─► capture ring ─► [low-cut, AGC, meter, VOX] ─► encoder ─► write-behind ring ─► card
//!                              chain::Chain                 encoder    ring::WriteBehind
//! ```
//!
//! * [`encoder`]: WAV PCM 16-bit ([`wav`]), IMA ADPCM in WAV ([`adpcm`]) and
//!   FLAC levels 0–2 with a fixed block size ([`flac`]). Each produces
//!   whole blocks into a caller's buffer and a header that can be
//!   rewritten in place while the file grows (every 10 s, so a power loss
//!   costs at most that much).
//! * [`chain`]: the per-block processing: first-order low-cut, a slow
//!   feed-forward AGC with a limiter ([`agc`]), the peak meter with hold
//!   and clip ([`meter`]) and voice activation with a hang time ([`vox`]).
//! * [`ring`]: the write-behind buffer between the encoder and the card:
//!   index arithmetic only (the bytes are the caller's), writes in whole
//!   sectors at file offsets that are multiples of 512, a bounded latency,
//!   and blocks that do not fit counted as drops, never lost silently.
//! * [`session`]: the recorder's state machine (start, pause, marks,
//!   automatic split, the time limit, card full at 1 MiB free, header
//!   rewrites) for the firmware and the simulator alike.
//! * [`name`]: `/Recordings/REC-YYYYMMDD-HHMMSS.ext`, or a counter when the
//!   clock is not set.
//! * [`pcm`]: the capture word layouts (24-bit samples in 32-bit FIFO
//!   words to 16-bit, in place).
//! * [`fake`]: a fake ADC (tones, noise, speech-like bursts) for tests and
//!   the simulator.
//!
//! `no_std`, no allocation, no `unsafe`: every buffer is the caller's.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod adpcm;
pub mod agc;
pub mod bits;
pub mod chain;
pub mod encoder;
pub mod fake;
pub mod flac;
pub mod meter;
pub mod name;
pub mod pcm;
pub mod ring;
pub mod session;
pub mod vox;
pub mod wav;

pub use encoder::{Encoder, Format, Params};

/// Recording sources (03 §13, docs/hw/audio.md §3.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    /// The built-in microphone (mono).
    #[default]
    Mic,
    /// The line input (stereo), if the board has one.
    Line,
    /// The FM tuner's output (stereo).
    Fm,
}

impl Source {
    /// Name used by the developer commands and the status line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Source::Mic => "mic",
            Source::Line => "line",
            Source::Fm => "fm",
        }
    }

    /// Parse a developer-command word.
    #[must_use]
    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "mic" => Some(Source::Mic),
            "line" => Some(Source::Line),
            "fm" => Some(Source::Fm),
            _ => None,
        }
    }

    /// Channels captured from this source: the microphone is mono (the
    /// vendor's default; stereo ADC is gated by the IC type), FM and the
    /// line input stereo.
    #[must_use]
    pub const fn channels(self) -> u8 {
        match self {
            Source::Mic => 1,
            Source::Line | Source::Fm => 2,
        }
    }
}

/// Recording quality (Settings › Recording › Quality): sample rate; the
/// channel count follows the source.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Quality {
    /// 16 kHz.
    Voice,
    /// 44.1 kHz.
    #[default]
    Standard,
    /// 48 kHz.
    High,
}

impl Quality {
    #[must_use]
    pub const fn rate(self) -> u32 {
        match self {
            Quality::Voice => 16_000,
            Quality::Standard => 44_100,
            Quality::High => 48_000,
        }
    }

    #[must_use]
    pub const fn from_index(i: i16) -> Quality {
        match i {
            0 => Quality::Voice,
            2 => Quality::High,
            _ => Quality::Standard,
        }
    }
}
