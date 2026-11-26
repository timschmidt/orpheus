//! `orpheus-synth`: the synthesis engine shared by MIDI playback, the
//! Sequencer, the games' sound effects, the signal generator and the speech
//! engine's resonators (docs/plan/04-audio.md §9; WPs SYN-1, SYN-2).
//!
//! ```ignore
//! use orpheus_synth::{Engine, Event, Param, presets};
//! let mut synth = Engine::new(48_000, 1);
//! synth.set_patch(0, &presets::SAW_LEAD);
//! let events = [
//!     Event::note_on(0, 0, 60, 100),          // part 0, C4, at sample 0
//!     Event::param(100, 0, Param::Cutoff, 900.0),
//!     Event::note_off(1000, 0, 60),
//! ];
//! let mut block = [0i16; 2 * 1152];           // interleaved stereo
//! synth.render_i16(&mut block, &events);      // events land on their sample
//! ```
//!
//! # Building blocks (usable on their own)
//!
//! * [`osc`]: band-limited saw and pulse (polyBLEP), triangle (polyBLAMP),
//!   sine, wavetables ([`wavetable`], mip-mapped), white and pink noise.
//! * [`fm`]: 2–4 operator FM with 8 algorithms and feedback.
//! * [`env`]: ADSR with linear or exponential segments (also the pitch
//!   envelope).
//! * [`filter`]: trapezoidal state-variable filter, low/band/high pass.
//! * [`lfo`]: sine, triangle, square, saw, sample-and-hold; Hz or tempo sync.
//! * [`drum`]: kick, snare, closed/open hat, clap, tom, rim, cowbell,
//!   cymbal, clave, shaker.
//! * [`kit`]: drum kits as data (note → drum or sample, choke groups).
//! * [`sample`], [`wav`]: sample playback from a bounded page cache fed by a
//!   block source (the card), WAV parsing.
//! * [`fx`]: delay, chorus, reverb (sends), soft limiter (master).
//! * [`bank`]: patches and kits in a compact binary format (sound banks).
//! * [`engine`]: [`PARTS`] parts, [`MAX_VOICES`] voices with stealing, mono
//!   legato with glide, sample-accurate events, mixing and rendering.
//! * [`bench`](mod@bench): the fixed loads of the player's `synth bench` (SYN-3).
//! * [`load`]: the engine's measured cost on the player and the load
//!   governor (effects, then voices, shed to stay inside a CPU budget).
//!
//! # Features
//!
//! `samples` (sample playback) and `delay` (the delay effect), both on by
//! default. The player's firmware builds without them (SYN-3: no card
//! sample source yet, no MIDI controller feeds the delay, and the code
//! window is full): sample notes then do not start, and the delay keeps its
//! settings and adds nothing.
//!
//! # Determinism
//!
//! Everything is `f32` with only IEEE basic operations (no fused
//! multiply-add, no `libm`), integer phase accumulators and seeded xorshift
//! noise, so a render is a pure function of the seed, the patches and the
//! events — bit-identical on the host, in the browser and on the player
//! ([`math`] states the one FPU requirement), at any optimisation level,
//! and independent of the block sizes the caller uses. The golden test
//! hashes the renders; the `render` example checks the same hashes in a
//! release build.
//!
//! # Memory
//!
//! All state lives in [`Engine`]; no allocation, no `unsafe`. An all-zero
//! `Engine` is a valid value (`bytemuck::Zeroable`) and [`Engine::init`]
//! builds it in place, so the player puts it in its decoder pool without
//! passing 62 KB through a task's stack. Sample data
//! lives in a [`SampleCache`] over memory the caller lends; the engine reads
//! it only during `render_*_with`. [`ram`] gives the sizes (the `ram_table`
//! test prints them).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod bank;
pub mod bench;
pub mod demo;
pub mod drum;
pub mod engine;
pub mod env;
pub mod filter;
pub mod fm;
pub mod fx;
pub mod kit;
pub mod lfo;
pub mod load;
pub mod math;
pub mod osc;
pub mod patch;
pub mod presets;
pub mod sample;
mod voice;
pub mod wav;
pub mod wavetable;

pub use drum::{DrumKind, DrumParams};
pub use engine::{
    CONTROL_PERIOD, Engine, Event, EventKind, FxParam, MAX_VOICES, PARTS, Param, TAILS, VoiceInfo,
};
pub use env::{Adsr, Curve};
pub use filter::FilterMode;
pub use fm::{FmParams, OpParams};
pub use fx::{ChorusParams, DelayParams, ReverbParams};
pub use kit::{Kit, KitEntry};
pub use lfo::{LfoParams, LfoRate, LfoWave};
pub use osc::{Osc, Waveform};
pub use patch::{FilterParams, Kind, OscParams, Patch, SubParams};
pub use sample::{SampleCache, SampleLoop, SampleParams, SampleSource, SliceSource};

/// RAM of the engine's parts, in bytes (`size_of`, this build).
pub mod ram {
    use core::mem::size_of;

    /// One voice slot (sources, envelopes, filter, LFO, gains).
    #[must_use]
    pub fn voice() -> usize {
        size_of::<crate::voice::Voice>()
    }

    /// One drum voice's source state (inside a voice slot).
    #[must_use]
    pub fn drum_voice() -> usize {
        size_of::<crate::drum::DrumVoice>()
    }

    /// One oscillator.
    #[must_use]
    pub fn osc() -> usize {
        size_of::<crate::osc::Osc>()
    }

    /// One patch.
    #[must_use]
    pub fn patch() -> usize {
        size_of::<crate::patch::Patch>()
    }

    /// The delay.
    #[must_use]
    pub fn delay() -> usize {
        size_of::<crate::fx::Delay>()
    }

    /// The chorus.
    #[must_use]
    pub fn chorus() -> usize {
        size_of::<crate::fx::Chorus>()
    }

    /// The reverb.
    #[must_use]
    pub fn reverb() -> usize {
        size_of::<crate::fx::Reverb>()
    }

    /// One kit slot.
    #[must_use]
    pub fn kit() -> usize {
        size_of::<crate::kit::Kit>()
    }

    /// A sample cache's own state (tags and slots; the page memory is
    /// borrowed: 512 bytes per page).
    #[must_use]
    pub fn sample_cache() -> usize {
        size_of::<crate::sample::SampleCache<'static>>()
    }

    /// One sample player (inside a voice slot).
    #[must_use]
    pub fn sample_player() -> usize {
        size_of::<crate::sample::SamplePlayer>()
    }

    /// The whole engine.
    #[must_use]
    pub fn engine() -> usize {
        size_of::<crate::engine::Engine>()
    }

    /// The engine without its effects (voices, parts, buses).
    #[must_use]
    pub fn core() -> usize {
        engine() - delay() - chorus() - reverb()
    }

    /// Read-only data: the wavetables.
    pub const WAVETABLE_ROM: usize =
        crate::wavetable::TABLES * crate::wavetable::LEVELS * crate::wavetable::LEN * 2;
}
