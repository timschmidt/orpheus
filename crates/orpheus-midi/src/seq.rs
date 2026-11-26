//! Channel messages to synthesis events: the state of the 16 channels and
//! what each message changes in the engine.
//!
//! Channel *n* plays on engine part *n*. Channel 10 (index 9) plays the
//! kit in kit slot 0; the others play the [`crate::gm`] sound of their
//! program. The controllers:
//!
//! | Message | Effect |
//! |---|---|
//! | note on/off | `NoteOn`/`NoteOff` (velocity 0 is an off); dropped on muted channels |
//! | program change | the program's sound (`set_patch`, only when the sound changes); on channel 10 the kit ([`crate::gm::kit_for`]) |
//! | pitch bend | `Bend` = value × RPN 0 range ÷ [`BEND_SPAN`] (patches are installed with a 24-semitone range) |
//! | CC 7 volume, 11 expression | `Volume` = (vol/127)² · (expr/127)² (the GM level curve, 40 log₁₀) |
//! | CC 10 pan | `Pan` = (value − 64)/63 |
//! | CC 1 modulation, channel pressure | `LfoPitch` = the patch's own vibrato + [`MOD_DEPTH`] · max(mod, pressure)/127 semitones |
//! | CC 64 sustain | `Sustain` (≥ 64 down) |
//! | CC 91 reverb, 93 chorus | `ReverbSend`, `ChorusSend` (value/127; when effects are on) |
//! | CC 100/101 RPN, 6/38 data entry, 96/97 increment | RPN 0: bend range (semitones and cents); others ignored; NRPN (98/99) deselects |
//! | CC 120 all sound off, 123–127 all notes off | `AllNotesOff` of the part |
//! | CC 121 reset all controllers | modulation, pressure, bend, expression, sustain, RPN to defaults (volume and pan stay, RP-015) |
//!
//! Polyphonic pressure and other controllers are ignored. The sequencer
//! can also keep its state without emitting anything (a seek scans the
//! file that way, remembering the notes held at the target), and then
//! [`Sequencer::restore`] emits the whole state at once; a song starts
//! with `restore` of the default state, so starting and seeking go
//! through the same code and leave the engine in the same state.

use orpheus_synth::{EventKind, Param};

use crate::gm;

/// The drum channel (channel 10).
pub const DRUM_CHANNEL: u8 = 9;
/// Pitch-bend range of every installed patch, semitones; the `Bend`
/// parameter is scaled to the channel's RPN range within it.
pub const BEND_SPAN: f32 = 24.0;
/// Vibrato added by full modulation (or pressure), semitones.
pub const MOD_DEPTH: f32 = 0.6;
/// Notes remembered as held while seeking (re-struck at the target).
pub const HELD_MAX: usize = 32;
/// "No sound installed yet" in [`Sequencer::sound`].
pub const NO_SOUND: u8 = 0xFF;
/// The kit patch's sound index.
pub const KIT_SOUND: u8 = 0xFE;

/// What the player does for a message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// An engine event at the message's frame.
    Engine(EventKind),
    /// Install sound `sound` ([`crate::gm::SET`] index or [`KIT_SOUND`]) on
    /// `part` (with [`BEND_SPAN`] as its bend range).
    Sound {
        /// Part.
        part: u8,
        /// Sound.
        sound: u8,
    },
    /// Install factory kit `kit` in kit slot 0.
    Kit(u8),
}

/// One channel's controller state.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// Program, 0–127.
    pub program: u8,
    /// CC 7.
    pub volume: u8,
    /// CC 11.
    pub expression: u8,
    /// CC 10.
    pub pan: u8,
    /// CC 1.
    pub modulation: u8,
    /// Channel pressure.
    pub pressure: u8,
    /// CC 64 ≥ 64.
    pub sustain: bool,
    /// Pitch bend, 0–16383 (8192 centre).
    pub bend: u16,
    /// Selected RPN (0x3FFF: none).
    pub rpn: u16,
    /// Bend range, cents.
    pub bend_range: u16,
    /// CC 91.
    pub reverb: u8,
    /// CC 93.
    pub chorus: u8,
}

impl Channel {
    /// Power-on state (GM defaults: volume 100, expression 127, centre pan,
    /// ±2 semitones, reverb send 40).
    pub const DEFAULT: Self = Self {
        program: 0,
        volume: 100,
        expression: 127,
        pan: 64,
        modulation: 0,
        pressure: 0,
        sustain: false,
        bend: 8192,
        rpn: 0x3FFF,
        bend_range: 200,
        reverb: 40,
        chorus: 0,
    };

    /// The part volume.
    #[must_use]
    pub fn gain(&self) -> f32 {
        let v = f32::from(self.volume) * (1.0 / 127.0);
        let e = f32::from(self.expression) * (1.0 / 127.0);
        v * v * (e * e)
    }

    /// The part pan, −1 to 1.
    #[must_use]
    pub fn pan_value(&self) -> f32 {
        ((f32::from(self.pan) - 64.0) * (1.0 / 63.0)).clamp(-1.0, 1.0)
    }

    /// The `Bend` parameter.
    #[must_use]
    pub fn bend_value(&self) -> f32 {
        let b = (f32::from(self.bend) - 8192.0) * (1.0 / 8192.0);
        let semis = f32::from(self.bend_range) * 0.01;
        (b * semis * (1.0 / BEND_SPAN)).clamp(-1.0, 1.0)
    }

    fn vibrato(&self, sound: u8) -> f32 {
        let base = if sound < gm::SOUNDS as u8 { gm::patch(sound).lfo.pitch } else { 0.0 };
        let m = self.modulation.max(self.pressure);
        base + f32::from(m) * (MOD_DEPTH / 127.0)
    }
}

/// A note held at the seek target.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Held {
    ch: u8,
    note: u8,
    vel: u8,
}

/// The 16 channels, mute/solo, and the notes held while scanning.
#[derive(bytemuck::Zeroable, Clone, Debug)]
pub struct Sequencer {
    ch: [Channel; 16],
    /// Sound installed per part (as far as the sequencer has asked).
    sound: [u8; 16],
    kit: u8,
    mute: u16,
    solo: u16,
    effects: bool,
    held: [Held; HELD_MAX],
    n_held: u8,
}

impl Sequencer {
    /// Default state; `effects`: reverb and chorus sends follow CC 91/93
    /// (off: sends stay 0).
    #[must_use]
    pub fn new(effects: bool) -> Self {
        Self {
            ch: [Channel::DEFAULT; 16],
            sound: [NO_SOUND; 16],
            kit: 0,
            mute: 0,
            solo: 0,
            effects,
            held: [Held::default(); HELD_MAX],
            n_held: 0,
        }
    }

    /// Back to the default state (mute and solo stay).
    pub fn reset(&mut self) {
        self.ch = [Channel::DEFAULT; 16];
        self.sound = [NO_SOUND; 16];
        self.kit = 0;
        self.n_held = 0;
    }

    /// Channel `c`'s state.
    #[must_use]
    pub fn channel(&self, c: u8) -> &Channel {
        &self.ch[usize::from(c & 15)]
    }

    /// The sound installed on part `c`.
    #[must_use]
    pub fn sound(&self, c: u8) -> u8 {
        self.sound[usize::from(c & 15)]
    }

    /// Mute channel `c`.
    pub fn set_mute(&mut self, c: u8, on: bool) {
        let b = 1u16 << (c & 15);
        if on { self.mute |= b } else { self.mute &= !b }
    }

    /// Solo channel `c` (when any channel is soloed, only soloed ones play).
    pub fn set_solo(&mut self, c: u8, on: bool) {
        let b = 1u16 << (c & 15);
        if on { self.solo |= b } else { self.solo &= !b }
    }

    /// Muted channels (bit per channel).
    #[must_use]
    pub fn mutes(&self) -> u16 {
        self.mute
    }

    /// Soloed channels (bit per channel).
    #[must_use]
    pub fn solos(&self) -> u16 {
        self.solo
    }

    /// Whether channel `c` sounds under the mute and solo settings.
    #[must_use]
    pub fn audible(&self, c: u8) -> bool {
        let b = 1u16 << (c & 15);
        self.mute & b == 0 && (self.solo == 0 || self.solo & b != 0)
    }

    fn sound_for(c: u8, program: u8) -> u8 {
        if c == DRUM_CHANNEL { KIT_SOUND } else { gm::sound_of(program) }
    }

    /// Handle a channel message. With `live` false only the state changes
    /// (and the held notes are tracked for [`Sequencer::restore`]).
    pub fn handle(&mut self, status: u8, d1: u8, d2: u8, live: bool, emit: &mut dyn FnMut(Action)) {
        let c = status & 15;
        let part = c;
        let i = usize::from(c);
        match status & 0xF0 {
            0x90 if d2 > 0 => {
                if !live {
                    self.hold(c, d1, d2);
                } else if self.audible(c) {
                    emit(Action::Engine(EventKind::NoteOn { part, note: d1, velocity: d2 }));
                }
            }
            0x80 | 0x90 => {
                if live {
                    emit(Action::Engine(EventKind::NoteOff { part, note: d1 }));
                } else {
                    self.unhold(c, d1);
                }
            }
            0xC0 => {
                self.ch[i].program = d1;
                if c == DRUM_CHANNEL {
                    let k = gm::kit_for(d1);
                    if k != self.kit {
                        self.kit = k;
                        if live {
                            emit(Action::Kit(k));
                        }
                    }
                } else if live {
                    let s = Self::sound_for(c, d1);
                    if s != self.sound[i] {
                        self.sound[i] = s;
                        emit(Action::Sound { part, sound: s });
                        emit(param(part, Param::LfoPitch, self.ch[i].vibrato(s)));
                    }
                }
            }
            0xD0 => {
                self.ch[i].pressure = d1;
                if live {
                    emit(param(part, Param::LfoPitch, self.ch[i].vibrato(self.sound[i])));
                }
            }
            0xE0 => {
                self.ch[i].bend = u16::from(d1) | (u16::from(d2) << 7);
                if live {
                    emit(param(part, Param::Bend, self.ch[i].bend_value()));
                }
            }
            0xB0 => self.control(c, d1, d2, live, emit),
            _ => {}
        }
    }

    fn control(&mut self, c: u8, cc: u8, v: u8, live: bool, emit: &mut dyn FnMut(Action)) {
        let part = c;
        if matches!(cc, 120 | 123..=127) {
            if live {
                emit(Action::Engine(EventKind::AllNotesOff { part }));
            } else {
                let n = usize::from(self.n_held);
                let mut k = 0;
                for i in 0..n.min(HELD_MAX) {
                    let h = self.held[i];
                    if h.ch != c {
                        if let Some(d) = self.held.get_mut(k) {
                            *d = h;
                        }
                        k += 1;
                    }
                }
                self.n_held = k as u8;
            }
            return;
        }
        let sound = self.sound[usize::from(c)];
        let effects = self.effects;
        let ch = &mut self.ch[usize::from(c)];
        let mut out = |a: Action| {
            if live {
                emit(a);
            }
        };
        match cc {
            1 => {
                ch.modulation = v;
                out(param(part, Param::LfoPitch, ch.vibrato(sound)));
            }
            6 | 38 | 96 | 97 => {
                if ch.rpn == 0 {
                    let (semis, cents) = (ch.bend_range / 100, ch.bend_range % 100);
                    ch.bend_range = match cc {
                        6 => u16::from(v.min(24)) * 100 + cents,
                        38 => semis * 100 + u16::from(v.min(99)),
                        96 => (ch.bend_range + 100).min(2400),
                        _ => ch.bend_range.saturating_sub(100),
                    }
                    .min(2400);
                    out(param(part, Param::Bend, ch.bend_value()));
                }
            }
            7 => {
                ch.volume = v;
                out(param(part, Param::Volume, ch.gain()));
            }
            10 => {
                ch.pan = v;
                out(param(part, Param::Pan, ch.pan_value()));
            }
            11 => {
                ch.expression = v;
                out(param(part, Param::Volume, ch.gain()));
            }
            64 => {
                let down = v >= 64;
                if down != ch.sustain {
                    ch.sustain = down;
                    out(param(part, Param::Sustain, if down { 1.0 } else { 0.0 }));
                }
            }
            91 => {
                ch.reverb = v;
                if effects {
                    out(param(part, Param::ReverbSend, f32::from(v) * (1.0 / 127.0)));
                }
            }
            93 => {
                ch.chorus = v;
                if effects {
                    out(param(part, Param::ChorusSend, f32::from(v) * (1.0 / 127.0)));
                }
            }
            98 | 99 => ch.rpn = 0x3FFF,
            100 => ch.rpn = (ch.rpn & 0x3F80) | u16::from(v),
            101 => ch.rpn = (ch.rpn & 0x007F) | (u16::from(v) << 7),
            121 => {
                ch.modulation = 0;
                ch.pressure = 0;
                ch.bend = 8192;
                ch.expression = 127;
                ch.rpn = 0x3FFF;
                out(param(part, Param::LfoPitch, ch.vibrato(sound)));
                out(param(part, Param::Bend, ch.bend_value()));
                out(param(part, Param::Volume, ch.gain()));
                if ch.sustain {
                    ch.sustain = false;
                    out(param(part, Param::Sustain, 0.0));
                }
            }
            _ => {}
        }
    }

    fn push_held(&mut self, h: Held) {
        let n = usize::from(self.n_held);
        if n == HELD_MAX {
            self.held.copy_within(1.., 0);
            self.held[HELD_MAX - 1] = h;
        } else if let Some(d) = self.held.get_mut(n) {
            *d = h;
            self.n_held += 1;
        }
    }

    fn hold(&mut self, ch: u8, note: u8, vel: u8) {
        self.unhold(ch, note);
        self.push_held(Held { ch, note, vel });
    }

    fn unhold(&mut self, ch: u8, note: u8) {
        let n = usize::from(self.n_held).min(HELD_MAX);
        if let Some(i) = self.held.iter().take(n).position(|h| h.ch == ch && h.note == note) {
            for k in i + 1..n {
                if let (Some(&h), Some(d)) = (self.held.get(k), k.checked_sub(1))
                    && let Some(d) = self.held.get_mut(d)
                {
                    *d = h;
                }
            }
            self.n_held -= 1;
        }
    }

    /// Notes held (from a silent scan), oldest first: (channel, note,
    /// velocity).
    pub fn held(&self) -> impl Iterator<Item = (u8, u8, u8)> + '_ {
        self.held[..usize::from(self.n_held)].iter().map(|h| (h.ch, h.note, h.vel))
    }

    /// Emit the whole state: every part's sound and controllers, the kit,
    /// then (with `chase`) the held melodic notes of audible channels.
    pub fn restore(&mut self, chase: bool, emit: &mut impl FnMut(Action)) {
        emit(Action::Kit(self.kit));
        for c in 0..16u8 {
            let ch = self.ch[usize::from(c)];
            let s = Self::sound_for(c, ch.program);
            self.sound[usize::from(c)] = s;
            emit(Action::Sound { part: c, sound: s });
            emit(param(c, Param::Volume, ch.gain()));
            emit(param(c, Param::Pan, ch.pan_value()));
            emit(param(c, Param::Bend, ch.bend_value()));
            emit(param(c, Param::LfoPitch, ch.vibrato(s)));
            emit(param(c, Param::Sustain, if ch.sustain { 1.0 } else { 0.0 }));
            let (rv, chs) = if self.effects {
                (f32::from(ch.reverb) * (1.0 / 127.0), f32::from(ch.chorus) * (1.0 / 127.0))
            } else {
                (0.0, 0.0)
            };
            emit(param(c, Param::ReverbSend, rv));
            emit(param(c, Param::ChorusSend, chs));
            emit(param(c, Param::DelaySend, 0.0));
        }
        if chase {
            for i in 0..usize::from(self.n_held).min(HELD_MAX) {
                let h = self.held[i];
                if h.ch != DRUM_CHANNEL && self.audible(h.ch) {
                    emit(Action::Engine(EventKind::NoteOn { part: h.ch, note: h.note, velocity: h.vel }));
                }
            }
        }
        self.n_held = 0;
    }
}

fn param(part: u8, param: Param, value: f32) -> Action {
    Action::Engine(EventKind::Param { part, param, value })
}
