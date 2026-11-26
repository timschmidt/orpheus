//! Writing Standard MIDI Files into a byte buffer (no allocation): the
//! demo songs, the tests' files, and later the Sequencer's export.
//!
//! Ticks are absolute; within a track they must not go back (an earlier
//! tick is written as a zero delta). Running status is used when enabled.
//! [`Writer::raw`] writes bytes as they are (the tests' broken files).

use crate::smf::Division;

/// A file being written.
#[derive(Debug)]
pub struct Writer<'a> {
    buf: &'a mut [u8],
    n: usize,
    full: bool,
    tracks: u16,
    /// Offset of the open track's length field and the track's last tick.
    open: Option<(usize, u64)>,
    running: bool,
    status: u8,
}

impl<'a> Writer<'a> {
    /// A file of `format` with `division` (the track count is filled in by
    /// [`Writer::finish`]).
    pub fn new(buf: &'a mut [u8], format: u16, division: Division) -> Self {
        let mut w = Self { buf, n: 0, full: false, tracks: 0, open: None, running: false, status: 0 };
        let d: u16 = match division {
            Division::Metrical(t) => t & 0x7FFF,
            Division::Smpte { fps, tpf } => {
                (u16::from((fps as i8).wrapping_neg() as u8) << 8) | u16::from(tpf)
            }
        };
        w.raw(b"MThd");
        w.raw(&6u32.to_be_bytes());
        w.raw(&format.to_be_bytes());
        w.raw(&[0, 0]);
        w.raw(&d.to_be_bytes());
        w
    }

    /// Use running status for channel messages from now on.
    pub fn running_status(&mut self, on: bool) {
        self.running = on;
    }

    /// Bytes as they are.
    pub fn raw(&mut self, b: &[u8]) {
        if self.n + b.len() > self.buf.len() {
            self.full = true;
            return;
        }
        self.buf[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
    }

    /// A variable-length number.
    pub fn varlen(&mut self, v: u32) {
        let v = v & 0x0FFF_FFFF;
        let mut tmp = [0u8; 4];
        let mut i = 4;
        let mut x = v;
        loop {
            i -= 1;
            tmp[i] = (x & 0x7F) as u8 | if i == 3 { 0 } else { 0x80 };
            x >>= 7;
            if x == 0 {
                break;
            }
        }
        self.raw(&tmp[i..]);
    }

    /// Start a track.
    pub fn track(&mut self) {
        self.close();
        self.raw(b"MTrk");
        let at = self.n;
        self.raw(&[0; 4]);
        self.open = Some((at, 0));
        self.status = 0;
        self.tracks += 1;
    }

    fn delta(&mut self, tick: u64) {
        let Some((at, last)) = self.open else { return };
        let d = tick.saturating_sub(last).min(0x0FFF_FFFF) as u32;
        self.varlen(d);
        self.open = Some((at, last.max(tick)));
    }

    /// A channel message at `tick`.
    pub fn channel(&mut self, tick: u64, status: u8, d1: u8, d2: u8) {
        self.delta(tick);
        if !(self.running && status == self.status) {
            self.raw(&[status]);
        }
        self.status = status;
        self.raw(&[d1 & 0x7F]);
        if !matches!(status & 0xF0, 0xC0 | 0xD0) {
            self.raw(&[d2 & 0x7F]);
        }
    }

    /// A meta event.
    pub fn meta(&mut self, tick: u64, ty: u8, data: &[u8]) {
        self.delta(tick);
        self.raw(&[0xFF, ty]);
        self.varlen(data.len() as u32);
        self.raw(data);
        self.status = 0;
    }

    /// A SysEx event (`data` after the `F0`).
    pub fn sysex(&mut self, tick: u64, data: &[u8]) {
        self.delta(tick);
        self.raw(&[0xF0]);
        self.varlen(data.len() as u32);
        self.raw(data);
        self.status = 0;
    }

    /// Set tempo, µs per quarter.
    pub fn tempo(&mut self, tick: u64, us: u32) {
        let b = us.to_be_bytes();
        self.meta(tick, 0x51, &b[1..]);
    }

    /// End the open track at `tick` (or its last event) and close it.
    pub fn end_track(&mut self, tick: u64) {
        if self.open.is_some() {
            self.meta(tick, 0x2F, &[]);
            self.close();
        }
    }

    /// Close the open track without an end-of-track event.
    pub fn close(&mut self) {
        if let Some((at, _)) = self.open.take()
            && !self.full
        {
            let len = (self.n - at - 4) as u32;
            self.buf[at..at + 4].copy_from_slice(&len.to_be_bytes());
        }
    }

    /// Close the file; its length (`None` if the buffer was too small).
    pub fn finish(mut self) -> Option<usize> {
        self.close();
        if self.full || self.buf.len() < 14 {
            return None;
        }
        self.buf[10..12].copy_from_slice(&self.tracks.to_be_bytes());
        Some(self.n)
    }
}

/// Events of one track gathered in any order and written sorted (notes
/// are easier to write as start and length).
#[derive(Clone, Debug)]
pub struct TrackBuf<const N: usize> {
    ev: [(u64, u32, [u8; 3]); N],
    n: usize,
    /// Events that did not fit.
    pub dropped: usize,
}

impl<const N: usize> Default for TrackBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TrackBuf<N> {
    /// Empty.
    #[must_use]
    pub const fn new() -> Self {
        Self { ev: [(0, 0, [0; 3]); N], n: 0, dropped: 0 }
    }

    /// A channel message at `tick`. At equal ticks note-offs go first, then
    /// the order of adding.
    pub fn push(&mut self, tick: u64, status: u8, d1: u8, d2: u8) {
        if self.n == N {
            self.dropped += 1;
            return;
        }
        let off = status & 0xF0 == 0x80 || (status & 0xF0 == 0x90 && d2 == 0);
        let order = (u32::from(!off) << 31) | self.n as u32;
        self.ev[self.n] = (tick, order, [status, d1, d2]);
        self.n += 1;
    }

    /// A note from `tick` for `len` ticks.
    pub fn note(&mut self, tick: u64, len: u64, ch: u8, note: u8, vel: u8) {
        self.push(tick, 0x90 | ch, note, vel);
        self.push(tick + len, 0x80 | ch, note, 0);
    }

    /// A controller.
    pub fn cc(&mut self, tick: u64, ch: u8, cc: u8, v: u8) {
        self.push(tick, 0xB0 | ch, cc, v);
    }

    /// A program change.
    pub fn program(&mut self, tick: u64, ch: u8, p: u8) {
        self.push(tick, 0xC0 | ch, p, 0);
    }

    /// Pitch bend, −8192 to 8191.
    pub fn bend(&mut self, tick: u64, ch: u8, v: i16) {
        let u = (i32::from(v) + 8192).clamp(0, 16383) as u16;
        self.push(tick, 0xE0 | ch, (u & 0x7F) as u8, (u >> 7) as u8);
    }

    /// The events as added: (tick, order key, bytes).
    #[must_use]
    pub fn events(&self) -> &[(u64, u32, [u8; 3])] {
        &self.ev[..self.n]
    }

    /// The last tick.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.ev[..self.n].iter().map(|e| e.0).max().unwrap_or(0)
    }

    /// Write the events (sorted) into the open track of `w`.
    pub fn write(&mut self, w: &mut Writer<'_>) {
        let ev = &mut self.ev[..self.n];
        ev.sort_unstable_by_key(|e| (e.0, e.1));
        for &(t, _, [s, a, b]) in ev.iter() {
            w.channel(t, s, a, b);
        }
    }
}
