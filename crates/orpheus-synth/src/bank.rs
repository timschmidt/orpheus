//! Sound banks: patches and kits in a compact binary format, for files on
//! the card (the Sequencer's sounds, user patches) and for the factory
//! sounds.
//!
//! ```text
//! bank    = "OSB1" record*
//! record  = tag:u8 len:u16 body[len]          (unknown tags are skipped)
//! tag 1   = patch: name[16] patch
//! tag 2   = kit:   name[16] n:u8 entry[n]      (entry: 9 bytes, crate::kit::KitEntry field order)
//! patch   = kind:u8 kind-fields filter adsr(amp) adsr(filter) adsr(pitch)
//!           pitch_env_amount lfo glide flags:u8 level velocity bend_range
//! ```
//!
//! Names are UTF-8, NUL-padded. Numbers are little-endian; every `f32` is
//! stored bit for bit (4 bytes), so a patch survives a round trip exactly
//! and a bank renders the same everywhere. A subtractive patch takes 165
//! bytes, a four-operator FM patch 259, a drum patch 138, a kit 1 + 9 per
//! entry (289 for 32); a record adds 19 (header and name).
//! Enumerations are one-byte codes (the declaration order of each enum).
//! Decoding checks every code and length and returns a sanitised patch
//! ([`Patch::sanitized`]), so a corrupt file cannot upset the engine; the
//! `bank_decode` fuzz target checks that.

use crate::drum::{DrumKind, DrumParams};
use crate::env::{Adsr, Curve};
use crate::filter::FilterMode;
use crate::fm::{FmParams, OPS, OpParams};
use crate::kit::{KIT_ENTRIES, Kit, KitEntry};
use crate::lfo::{LfoParams, LfoRate, LfoWave};
use crate::osc::Waveform;
use crate::patch::{FilterParams, Kind, OscParams, Patch, SubParams};
use crate::sample::{SampleLoop, SampleParams};

/// File magic.
pub const MAGIC: [u8; 4] = *b"OSB1";
/// Bytes of a name.
pub const NAME_LEN: usize = 16;
/// Record tag of a patch.
pub const TAG_PATCH: u8 = 1;
/// Record tag of a kit.
pub const TAG_KIT: u8 = 2;
/// Largest encoded patch, bytes.
pub const PATCH_MAX: usize = 260;

/// A decoding or encoding failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankError {
    /// The data ends inside a field or record.
    Short,
    /// No `OSB1` magic.
    BadMagic,
    /// An unknown enumeration code.
    BadCode,
    /// The output buffer is full.
    Full,
}

/// A record name (UTF-8, NUL-padded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name(pub [u8; NAME_LEN]);

impl Name {
    /// `s` cut to 16 bytes (at a character boundary).
    #[must_use]
    pub fn new(s: &str) -> Self {
        let mut n = [0u8; NAME_LEN];
        let mut end = s.len().min(NAME_LEN);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        n[..end].copy_from_slice(&s.as_bytes()[..end]);
        Self(n)
    }

    /// The name as text ("" if it is not UTF-8).
    #[must_use]
    pub fn as_str(&self) -> &str {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.0[..end]).unwrap_or("")
    }
}

struct Out<'a> {
    buf: &'a mut [u8],
    n: usize,
}

impl Out<'_> {
    fn u8(&mut self, v: u8) -> Result<(), BankError> {
        *self.buf.get_mut(self.n).ok_or(BankError::Full)? = v;
        self.n += 1;
        Ok(())
    }

    fn bytes(&mut self, b: &[u8]) -> Result<(), BankError> {
        let dst = self.buf.get_mut(self.n..self.n + b.len()).ok_or(BankError::Full)?;
        dst.copy_from_slice(b);
        self.n += b.len();
        Ok(())
    }

    fn f(&mut self, v: f32) -> Result<(), BankError> {
        self.bytes(&v.to_le_bytes())
    }

    fn adsr(&mut self, e: &Adsr) -> Result<(), BankError> {
        for v in [e.attack, e.decay, e.sustain, e.release] {
            self.f(v)?;
        }
        self.u8(match e.curve {
            Curve::Linear => 0,
            Curve::Exponential => 1,
        })
    }
}

struct In<'a> {
    d: &'a [u8],
    n: usize,
}

impl In<'_> {
    fn u8(&mut self) -> Result<u8, BankError> {
        let v = *self.d.get(self.n).ok_or(BankError::Short)?;
        self.n += 1;
        Ok(v)
    }

    fn f(&mut self) -> Result<f32, BankError> {
        let b = self.d.get(self.n..self.n + 4).ok_or(BankError::Short)?;
        self.n += 4;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn adsr(&mut self) -> Result<Adsr, BankError> {
        let (attack, decay, sustain, release) = (self.f()?, self.f()?, self.f()?, self.f()?);
        let curve = match self.u8()? {
            0 => Curve::Linear,
            1 => Curve::Exponential,
            _ => return Err(BankError::BadCode),
        };
        Ok(Adsr { attack, decay, sustain, release, curve })
    }
}

fn wave_code(w: Waveform) -> (u8, u8) {
    match w {
        Waveform::Sine => (0, 0),
        Waveform::Saw => (1, 0),
        Waveform::Pulse => (2, 0),
        Waveform::Triangle => (3, 0),
        Waveform::Table(t) => (4, t),
        Waveform::White => (5, 0),
        Waveform::Pink => (6, 0),
    }
}

fn wave_from(c: u8, t: u8) -> Result<Waveform, BankError> {
    Ok(match c {
        0 => Waveform::Sine,
        1 => Waveform::Saw,
        2 => Waveform::Pulse,
        3 => Waveform::Triangle,
        4 => Waveform::Table(t),
        5 => Waveform::White,
        6 => Waveform::Pink,
        _ => return Err(BankError::BadCode),
    })
}

const FILTER_MODES: [FilterMode; 4] =
    [FilterMode::Off, FilterMode::LowPass, FilterMode::BandPass, FilterMode::HighPass];
const LFO_WAVES: [LfoWave; 5] =
    [LfoWave::Sine, LfoWave::Triangle, LfoWave::Square, LfoWave::Saw, LfoWave::SampleHold];

fn code_of<T: PartialEq + Copy>(table: &[T], v: T) -> u8 {
    table.iter().position(|&x| x == v).unwrap_or(0) as u8
}

fn from_code<T: Copy>(table: &[T], c: u8) -> Result<T, BankError> {
    table.get(usize::from(c)).copied().ok_or(BankError::BadCode)
}

/// Encode `p` into `buf`; returns the bytes written (at most
/// [`PATCH_MAX`]).
///
/// # Errors
///
/// [`BankError::Full`] when `buf` is too small.
pub fn encode_patch(p: &Patch, buf: &mut [u8]) -> Result<usize, BankError> {
    let mut o = Out { buf, n: 0 };
    match &p.kind {
        Kind::Subtractive(s) => {
            o.u8(0)?;
            for q in &s.osc {
                let (c, t) = wave_code(q.wave);
                o.u8(c)?;
                o.u8(t)?;
                for v in [q.semitones, q.cents, q.level, q.width, q.phase] {
                    o.f(v)?;
                }
            }
            o.f(s.noise)?;
        }
        Kind::Fm(f) => {
            o.u8(1)?;
            o.u8(f.algorithm)?;
            o.u8(f.ops)?;
            o.f(f.feedback)?;
            o.f(f.key_scale)?;
            for op in &f.op {
                for v in [op.ratio, op.detune, op.level, op.velocity] {
                    o.f(v)?;
                }
                o.adsr(&op.env)?;
            }
        }
        Kind::Drum(d) => {
            o.u8(2)?;
            o.u8(d.kind.code())?;
            for v in [d.tune, d.decay, d.tone, d.snap, d.level] {
                o.f(v)?;
            }
        }
        Kind::Kit(slot) => {
            o.u8(3)?;
            o.u8(*slot)?;
        }
        Kind::Sample(s) => {
            o.u8(4)?;
            o.u8(s.slot)?;
            o.u8(s.root)?;
            o.f(s.tune)?;
            o.u8(s.looping.code())?;
            o.u8(u8::from(s.one_shot))?;
            o.f(s.start)?;
        }
    }
    let f = &p.filter;
    o.u8(code_of(&FILTER_MODES, f.mode))?;
    for v in [f.cutoff, f.resonance, f.env_amount, f.key_track, f.velocity] {
        o.f(v)?;
    }
    o.adsr(&p.amp_env)?;
    o.adsr(&p.filter_env)?;
    o.adsr(&p.pitch_env)?;
    o.f(p.pitch_env_amount)?;
    let l = &p.lfo;
    o.u8(code_of(&LFO_WAVES, l.wave))?;
    match l.rate {
        LfoRate::Hz(v) => {
            o.u8(0)?;
            o.f(v)?;
        }
        LfoRate::Sync(v) => {
            o.u8(1)?;
            o.f(v)?;
        }
    }
    for v in [l.pitch, l.cutoff, l.amp, l.width] {
        o.f(v)?;
    }
    o.u8(u8::from(l.key_sync))?;
    o.f(p.glide)?;
    o.u8(u8::from(p.mono) | u8::from(p.legato) << 1)?;
    for v in [p.level, p.velocity, p.bend_range] {
        o.f(v)?;
    }
    Ok(o.n)
}

/// Decode a patch from the start of `d`; returns it (sanitised) and the
/// bytes used.
///
/// # Errors
///
/// [`BankError::Short`] or [`BankError::BadCode`].
pub fn decode_patch(d: &[u8]) -> Result<(Patch, usize), BankError> {
    let mut i = In { d, n: 0 };
    let kind = match i.u8()? {
        0 => {
            let mut osc = [OscParams::OFF; 2];
            for q in &mut osc {
                let (c, t) = (i.u8()?, i.u8()?);
                *q = OscParams {
                    wave: wave_from(c, t)?,
                    semitones: i.f()?,
                    cents: i.f()?,
                    level: i.f()?,
                    width: i.f()?,
                    phase: i.f()?,
                };
            }
            Kind::Subtractive(SubParams { osc, noise: i.f()? })
        }
        1 => {
            let (algorithm, ops, feedback, key_scale) = (i.u8()?, i.u8()?, i.f()?, i.f()?);
            let mut op = [OpParams::OFF; OPS];
            for o in &mut op {
                *o = OpParams {
                    ratio: i.f()?,
                    detune: i.f()?,
                    level: i.f()?,
                    velocity: i.f()?,
                    env: i.adsr()?,
                };
            }
            Kind::Fm(FmParams { algorithm, ops, feedback, key_scale, op })
        }
        2 => {
            let kind = DrumKind::from_code(i.u8()?).ok_or(BankError::BadCode)?;
            Kind::Drum(DrumParams {
                kind,
                tune: i.f()?,
                decay: i.f()?,
                tone: i.f()?,
                snap: i.f()?,
                level: i.f()?,
            })
        }
        3 => Kind::Kit(i.u8()?),
        4 => {
            let (slot, root, tune) = (i.u8()?, i.u8()?, i.f()?);
            let looping = SampleLoop::from_code(i.u8()?).ok_or(BankError::BadCode)?;
            let one_shot = match i.u8()? {
                0 => false,
                1 => true,
                _ => return Err(BankError::BadCode),
            };
            Kind::Sample(SampleParams { slot, root, tune, looping, one_shot, start: i.f()? })
        }
        _ => return Err(BankError::BadCode),
    };
    let filter = FilterParams {
        mode: from_code(&FILTER_MODES, i.u8()?)?,
        cutoff: i.f()?,
        resonance: i.f()?,
        env_amount: i.f()?,
        key_track: i.f()?,
        velocity: i.f()?,
    };
    let (amp_env, filter_env, pitch_env) = (i.adsr()?, i.adsr()?, i.adsr()?);
    let pitch_env_amount = i.f()?;
    let wave = from_code(&LFO_WAVES, i.u8()?)?;
    let rate = match i.u8()? {
        0 => LfoRate::Hz(i.f()?),
        1 => LfoRate::Sync(i.f()?),
        _ => return Err(BankError::BadCode),
    };
    let (pitch, cutoff, amp, width) = (i.f()?, i.f()?, i.f()?, i.f()?);
    let key_sync = i.u8()? != 0;
    let glide = i.f()?;
    let flags = i.u8()?;
    let p = Patch {
        kind,
        filter,
        amp_env,
        filter_env,
        pitch_env,
        pitch_env_amount,
        lfo: LfoParams { wave, rate, pitch, cutoff, amp, width, key_sync },
        glide,
        mono: flags & 1 != 0,
        legato: flags & 2 != 0,
        level: i.f()?,
        velocity: i.f()?,
        bend_range: i.f()?,
    };
    Ok((p.sanitized(), i.n))
}

/// A bank being written into a caller's buffer.
#[derive(Debug)]
pub struct Writer<'a> {
    buf: &'a mut [u8],
    n: usize,
}

impl<'a> Writer<'a> {
    /// Start a bank in `buf` (writes the magic).
    ///
    /// # Errors
    ///
    /// [`BankError::Full`].
    pub fn new(buf: &'a mut [u8]) -> Result<Self, BankError> {
        buf.get_mut(..4).ok_or(BankError::Full)?.copy_from_slice(&MAGIC);
        Ok(Self { buf, n: 4 })
    }

    /// Bytes written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Whether only the magic is written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n <= 4
    }

    fn record(
        &mut self,
        tag: u8,
        name: &str,
        body: impl FnOnce(&mut [u8]) -> Result<usize, BankError>,
    ) -> Result<(), BankError> {
        let start = self.n;
        let head = self.buf.get_mut(start..start + 3 + NAME_LEN).ok_or(BankError::Full)?;
        head[0] = tag;
        head[3..].copy_from_slice(&Name::new(name).0);
        let rest = &mut self.buf[start + 3 + NAME_LEN..];
        let n = NAME_LEN + body(rest)?;
        let len = u16::try_from(n).map_err(|_| BankError::Full)?;
        self.buf[start + 1..start + 3].copy_from_slice(&len.to_le_bytes());
        self.n = start + 3 + n;
        Ok(())
    }

    /// Append a patch record.
    ///
    /// # Errors
    ///
    /// [`BankError::Full`].
    pub fn patch(&mut self, name: &str, p: &Patch) -> Result<(), BankError> {
        self.record(TAG_PATCH, name, |b| encode_patch(p, b))
    }

    /// Append a kit record.
    ///
    /// # Errors
    ///
    /// [`BankError::Full`].
    pub fn kit(&mut self, name: &str, k: &Kit) -> Result<(), BankError> {
        self.record(TAG_KIT, name, |b| {
            let e = k.entries();
            let mut o = Out { buf: b, n: 0 };
            o.u8(e.len() as u8)?;
            for x in e {
                o.bytes(&[
                    x.note,
                    x.sound,
                    x.choke,
                    x.pan as u8,
                    x.tune as u8,
                    x.decay,
                    x.tone,
                    x.snap,
                    x.level,
                ])?;
            }
            Ok(o.n)
        })
    }
}

/// A record of a bank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Record {
    /// A patch.
    Patch(Name, Patch),
    /// A kit.
    Kit(Name, Kit),
}

/// The records of a bank, in order (unknown tags skipped). An error ends
/// the iteration.
#[derive(Clone, Debug)]
pub struct Records<'a> {
    d: &'a [u8],
    n: usize,
    failed: bool,
}

/// Iterate over the records of the bank `d`.
///
/// # Errors
///
/// [`BankError::BadMagic`] when `d` is not a bank.
pub fn records(d: &[u8]) -> Result<Records<'_>, BankError> {
    if d.get(..4) != Some(&MAGIC[..]) {
        return Err(BankError::BadMagic);
    }
    Ok(Records { d, n: 4, failed: false })
}

fn decode_kit(b: &[u8]) -> Result<Kit, BankError> {
    let n = usize::from(*b.first().ok_or(BankError::Short)?);
    if n > KIT_ENTRIES {
        return Err(BankError::BadCode);
    }
    let raw = b.get(1..1 + 9 * n).ok_or(BankError::Short)?;
    let mut e = [KitEntry::drum(0, DrumKind::Kick); KIT_ENTRIES];
    for (x, r) in e.iter_mut().zip(raw.chunks_exact(9)) {
        *x = KitEntry {
            note: r[0],
            sound: r[1],
            choke: r[2],
            pan: r[3] as i8,
            tune: r[4] as i8,
            decay: r[5],
            tone: r[6],
            snap: r[7],
            level: r[8],
        };
        if x.decode().is_none() {
            return Err(BankError::BadCode);
        }
    }
    Ok(Kit::new(&e[..n]))
}

impl Iterator for Records<'_> {
    type Item = Result<Record, BankError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.failed || self.n >= self.d.len() {
                return None;
            }
            let r = (|| {
                let h = self.d.get(self.n..self.n + 3).ok_or(BankError::Short)?;
                let (tag, len) = (h[0], usize::from(u16::from_le_bytes([h[1], h[2]])));
                let body = self.d.get(self.n + 3..self.n + 3 + len).ok_or(BankError::Short)?;
                self.n += 3 + len;
                if tag != TAG_PATCH && tag != TAG_KIT {
                    return Ok(None);
                }
                let name = Name(
                    body.get(..NAME_LEN).ok_or(BankError::Short)?.try_into().map_err(|_| BankError::Short)?,
                );
                let rest = &body[NAME_LEN..];
                Ok(Some(if tag == TAG_PATCH {
                    Record::Patch(name, decode_patch(rest)?.0)
                } else {
                    Record::Kit(name, decode_kit(rest)?)
                }))
            })();
            match r {
                Ok(None) => {}
                Ok(Some(rec)) => return Some(Ok(rec)),
                Err(e) => {
                    self.failed = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn factory_sounds_round_trip_exactly() {
        let mut buf = [0u8; 8192];
        let mut w = Writer::new(&mut buf).unwrap();
        for (name, p) in crate::presets::MELODIC {
            w.patch(name, &p).unwrap();
        }
        for (name, d) in crate::presets::DRUMS {
            w.patch(name, &Patch::drum(d)).unwrap();
        }
        w.patch("sampler", &crate::presets::SAMPLER).unwrap();
        for (name, k) in crate::kit::FACTORY {
            w.kit(name, &Kit::new(k)).unwrap();
        }
        let n = w.len();
        let mut got = records(&buf[..n]).unwrap();
        for (name, p) in crate::presets::MELODIC {
            assert_eq!(got.next(), Some(Ok(Record::Patch(Name::new(name), p.sanitized()))), "{name}");
        }
        for (name, d) in crate::presets::DRUMS {
            assert_eq!(got.next(), Some(Ok(Record::Patch(Name::new(name), Patch::drum(d).sanitized()))));
        }
        assert!(matches!(got.next(), Some(Ok(Record::Patch(_, _)))));
        for (name, k) in crate::kit::FACTORY {
            assert_eq!(got.next(), Some(Ok(Record::Kit(Name::new(name), Kit::new(k)))));
        }
        assert_eq!(got.next(), None);
        // Sizes quoted in the module docs.
        let mut b = [0u8; PATCH_MAX];
        assert_eq!(encode_patch(&crate::presets::SAW_LEAD, &mut b), Ok(165));
        assert_eq!(encode_patch(&crate::presets::FM_EPIANO, &mut b), Ok(259));
        assert_eq!(encode_patch(&Patch::KIT, &mut b), Ok(118));
        assert_eq!(encode_patch(&Patch::drum(DrumParams::new(DrumKind::Kick)), &mut b), Ok(138));
    }

    #[test]
    fn truncated_and_corrupt_banks_fail_cleanly() {
        let mut buf = [0u8; 1024];
        let mut w = Writer::new(&mut buf).unwrap();
        w.patch("lead", &crate::presets::SAW_LEAD).unwrap();
        w.kit("std", &Kit::new(crate::kit::STANDARD)).unwrap();
        let n = w.len();
        for cut in 4..n {
            for r in records(&buf[..cut]).unwrap() {
                let _ = r;
            }
        }
        assert_eq!(records(b"OSB2").err(), Some(BankError::BadMagic));
        let mut bad = buf;
        bad[4 + 3 + NAME_LEN] = 9; // kind code
        assert_eq!(records(&bad[..n]).unwrap().next(), Some(Err(BankError::BadCode)));
        let mut small = [0u8; 40];
        let mut w = Writer::new(&mut small).unwrap();
        assert_eq!(w.patch("x", &crate::presets::SAW_LEAD), Err(BankError::Full));
    }
}
