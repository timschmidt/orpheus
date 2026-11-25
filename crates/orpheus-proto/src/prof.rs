//! Profiler block (WP-T-8, docs/plan/07-tooling-testing.md §5.6): the PC
//! sampler's and the pager fault log's ring, 2 KiB at [`BASE`] in SRAM3.
//!
//! One capture at a time: either **PC samples** (the timer T2 interrupt
//! records the interrupted PC) or **page faults** (the MemManage handler
//! records each fault it served). The firmware starts and stops a capture
//! with the test commands `prof pc on|burst|off`, `prof faults on|burst|off`
//! and reports it with `prof status`; the host reads the whole block with
//! PEEK (`REQ_PEEK`, word reads) after the capture stopped and parses it
//! with [`Capture`].
//!
//! The ring holds whole events in order. When it is full a **spread**
//! capture (`on`) keeps every other entry and halves the rate at which it
//! takes new ones (the stride doubles), so whatever the length of the
//! capture the ring holds a uniform sample of all of it: entry `i` is event
//! number `i << shift`. A **burst** capture (`burst`) stops taking entries
//! when full instead and keeps counting events (`seen - count` were lost).
//!
//! | Word | Content |
//! |---|---|
//! | 0 | magic `"YPRF"` |
//! | 1 | mode: kind (bits 0–1: 1 PC, 2 faults), [`RUNNING`], [`BURST`], [`FULL`] |
//! | 2 | capacity in entries |
//! | 3 | entries stored |
//! | 4 | events seen since the start (PC ticks or faults) |
//! | 5 | `shift`: log2 of the stride |
//! | 6 | start time (T0, µs, wraps every 71 min) |
//! | 7 | stop time (T0, µs; 0 while running) |
//! | 8 | PC sampler period in µs (0 for faults) |
//! | 9 | layout [`VERSION`] |
//! | 10–11 | reserved (0) |
//! | 12… | entries |
//!
//! A **PC entry** is one word: the interrupted PC with bit 0 set when the
//! CPU was in handler mode (an interrupt, the RT executor; Thumb PCs are
//! even, so the bit is free). A **fault entry** is two words
//! ([`encode_fault`]): the fault address, the PC of the faulting
//! instruction, whether the line was read from NOR or only re-mapped,
//! whether it was a data access, and the time since the start in units of
//! 1,024 µs (18 bits: wraps after 268 s; the host unwraps).
//!
//! The code here runs inside the MemManage handler and the sampler
//! interrupt: no panics (every index is checked), no loops but the
//! thinning, no formatting.

use crate::{magic, rd32};

/// Profiler block: SRAM3, the 2 KiB after the PCM ring
/// ([`crate::mem::SRAM3_PROF`]).
pub const BASE: u32 = crate::mem::SRAM3_PROF;
pub const SIZE: usize = crate::mem::SRAM3_PROF_SIZE as usize;
pub const WORDS: usize = SIZE / 4;
pub const MAGIC: u32 = magic(b"YPRF");
pub const VERSION: u32 = 1;

pub const W_MAGIC: usize = 0;
pub const W_MODE: usize = 1;
pub const W_CAP: usize = 2;
pub const W_COUNT: usize = 3;
pub const W_SEEN: usize = 4;
pub const W_SHIFT: usize = 5;
pub const W_START: usize = 6;
pub const W_STOP: usize = 7;
pub const W_PERIOD: usize = 8;
pub const W_VERSION: usize = 9;
/// Header words; entries start here.
pub const HEADER: usize = 12;

/// Mode word: what is captured (bits 0–1).
pub const KIND_MASK: u32 = 0b11;
pub const KIND_PC: u32 = 1;
pub const KIND_FAULTS: u32 = 2;
/// Taking events.
pub const RUNNING: u32 = 1 << 4;
/// Stop taking entries when full (no thinning).
pub const BURST: u32 = 1 << 5;
/// A burst capture filled the ring.
pub const FULL: u32 = 1 << 6;

/// Words per entry.
pub const PC_WORDS: usize = 1;
pub const FAULT_WORDS: usize = 2;
/// Entries the block holds (even, so that thinning keeps exactly half).
pub const PC_CAP: u32 = ((WORDS - HEADER) / PC_WORDS) as u32 & !1;
pub const FAULT_CAP: u32 = ((WORDS - HEADER) / FAULT_WORDS) as u32 & !1;

/// Fault entry flags (word 0).
pub const F_LOADED: u32 = 1 << 22;
pub const F_DATA: u32 = 1 << 23;
/// Address and PC bits kept (`0x000000–0x3FFFFF`: SRAM0, the code and
/// rodata windows).
pub const ADDR_MASK: u32 = 0x003F_FFFF;
/// Fault time unit: `1 << TIME_SHIFT` µs.
pub const TIME_SHIFT: u32 = 10;
pub const TIME_BITS: u32 = 18;
pub const TIME_US: u32 = 1 << TIME_SHIFT;

/// Bit 0 of a PC entry: the CPU was in handler mode.
pub const PC_HANDLER: u32 = 1;

/// The block as the firmware sees it.
pub type Block = [u32; WORDS];

/// Start a capture: clears the header (entries are left as they are) and
/// sets the mode last.
pub fn start(b: &mut Block, kind: u32, burst: bool, period_us: u32, now_us: u32) {
    b[W_MODE] = 0;
    b[W_MAGIC] = MAGIC;
    b[W_CAP] = if kind == KIND_PC { PC_CAP } else { FAULT_CAP };
    b[W_COUNT] = 0;
    b[W_SEEN] = 0;
    b[W_SHIFT] = 0;
    b[W_START] = now_us;
    b[W_STOP] = 0;
    b[W_PERIOD] = period_us;
    b[W_VERSION] = VERSION;
    b[10] = 0;
    b[11] = 0;
    b[W_MODE] = (kind & KIND_MASK) | RUNNING | if burst { BURST } else { 0 };
}

/// Stop the capture (keeps what it holds).
pub fn stop(b: &mut Block, now_us: u32) {
    if b[W_MODE] & RUNNING != 0 {
        b[W_MODE] &= !RUNNING;
        b[W_STOP] = now_us;
    }
}

/// Whether a capture of `kind` is taking events.
#[inline(always)]
#[must_use]
pub fn running(b: &Block, kind: u32) -> bool {
    b[W_MODE] & (KIND_MASK | RUNNING) == kind | RUNNING
}

/// Offer one event (one entry of `e.len()` words: [`PC_WORDS`] or
/// [`FAULT_WORDS`]). Returns whether it was stored. One copy for both
/// kinds: it is resident (the sampler and the fault path call it).
#[inline(never)]
pub fn push(b: &mut Block, e: &[u32]) -> bool {
    let n = e.len();
    let seen = b[W_SEEN];
    b[W_SEEN] = seen.wrapping_add(1);
    let shift = b[W_SHIFT] & 31;
    if seen & ((1u32 << shift) - 1) != 0 {
        return false;
    }
    let mut count = b[W_COUNT];
    let cap = b[W_CAP].min(((WORDS - HEADER) / n.max(1)) as u32);
    if count >= cap {
        if b[W_MODE] & BURST != 0 || shift >= 31 {
            b[W_MODE] |= FULL;
            return false;
        }
        // Keep entries 0, 2, 4, …: entry `i` becomes event `i << shift`
        // with the doubled stride.
        let keep = count.div_ceil(2);
        for i in 1..keep as usize {
            for k in 0..n {
                let v = b.get(HEADER + 2 * i * n + k).copied().unwrap_or(0);
                if let Some(d) = b.get_mut(HEADER + i * n + k) {
                    *d = v;
                }
            }
        }
        count = keep;
        b[W_COUNT] = count;
        b[W_SHIFT] = shift + 1;
        if seen & ((2u32 << shift) - 1) != 0 {
            return false;
        }
    }
    let at = HEADER + count as usize * n;
    for (k, &v) in e.iter().enumerate() {
        if let Some(d) = b.get_mut(at + k) {
            *d = v;
        }
    }
    b[W_COUNT] = count + 1;
    true
}

/// A PC entry.
#[inline(always)]
#[must_use]
pub const fn encode_pc(pc: u32, handler: bool) -> u32 {
    (pc & !1) | handler as u32
}

/// A fault entry: fault address, faulting PC, NOR read or re-map only,
/// data access or instruction fetch, µs since the capture started.
#[inline(always)]
#[must_use]
pub const fn encode_fault(addr: u32, pc: u32, loaded: bool, data: bool, since_us: u32) -> [u32; 2] {
    let t = since_us >> TIME_SHIFT;
    [
        (addr & ADDR_MASK)
            | if loaded { F_LOADED } else { 0 }
            | if data { F_DATA } else { 0 }
            | ((t & 0xFF) << 24),
        (pc & ADDR_MASK) | (((t >> 8) & 0x3FF) << 22),
    ]
}

/// A decoded fault entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub addr: u32,
    pub pc: u32,
    /// The line was read from NOR (else only re-mapped).
    pub loaded: bool,
    /// A data access (`MMARVALID`): rodata, or a literal pool in a code
    /// page; else an instruction fetch.
    pub data: bool,
    /// Time since the start in [`TIME_US`] units, modulo `2^18`.
    pub t: u32,
}

#[must_use]
pub const fn decode_fault(w0: u32, w1: u32) -> Fault {
    Fault {
        addr: w0 & ADDR_MASK,
        pc: w1 & ADDR_MASK,
        loaded: w0 & F_LOADED != 0,
        data: w0 & F_DATA != 0,
        t: (w0 >> 24) | ((w1 >> 22) << 8),
    }
}

/// A decoded PC sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcSample {
    pub pc: u32,
    pub handler: bool,
}

#[must_use]
pub const fn decode_pc(w: u32) -> PcSample {
    PcSample { pc: w & !1, handler: w & PC_HANDLER != 0 }
}

/// A capture read back from the block.
#[derive(Clone, Copy, Debug)]
pub struct Capture<'a> {
    pub kind: u32,
    pub running: bool,
    pub burst: bool,
    pub full: bool,
    pub cap: u32,
    pub count: u32,
    pub seen: u32,
    pub shift: u32,
    pub start_us: u32,
    pub stop_us: u32,
    pub period_us: u32,
    entries: &'a [u8],
}

/// Why a block does not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Short,
    Magic,
    Version(u32),
    Kind(u32),
    /// More entries than the block holds.
    Count(u32),
}

impl<'a> Capture<'a> {
    /// Parse a block read from [`BASE`] (`SIZE` bytes, little-endian).
    pub fn parse(b: &'a [u8]) -> Result<Self, ParseError> {
        let w = |i: usize| rd32(b, 4 * i).ok_or(ParseError::Short);
        if w(W_MAGIC)? != MAGIC {
            return Err(ParseError::Magic);
        }
        let v = w(W_VERSION)?;
        if v != VERSION {
            return Err(ParseError::Version(v));
        }
        let mode = w(W_MODE)?;
        let kind = mode & KIND_MASK;
        let per = match kind {
            KIND_PC => PC_WORDS,
            KIND_FAULTS => FAULT_WORDS,
            k => return Err(ParseError::Kind(k)),
        };
        let count = w(W_COUNT)?;
        let data = b.get(4 * HEADER..).ok_or(ParseError::Short)?;
        if count as usize * per * 4 > data.len() {
            return Err(ParseError::Count(count));
        }
        Ok(Self {
            kind,
            running: mode & RUNNING != 0,
            burst: mode & BURST != 0,
            full: mode & FULL != 0,
            cap: w(W_CAP)?,
            count,
            seen: w(W_SEEN)?,
            shift: w(W_SHIFT)? & 31,
            start_us: w(W_START)?,
            stop_us: w(W_STOP)?,
            period_us: w(W_PERIOD)?,
            entries: &data[..count as usize * per * 4],
        })
    }

    /// Events each entry stands for (`2^shift`).
    #[must_use]
    pub fn stride(&self) -> u32 {
        1 << self.shift
    }

    /// Length of the capture in µs (stop − start; 0 while running).
    #[must_use]
    pub fn span_us(&self) -> u32 {
        if self.running { 0 } else { self.stop_us.wrapping_sub(self.start_us) }
    }

    /// The PC samples (empty for a fault capture).
    pub fn pc_samples(&self) -> impl Iterator<Item = PcSample> + 'a {
        let n = if self.kind == KIND_PC { self.entries.len() } else { 0 };
        self.entries[..n].chunks_exact(4).map(|c| decode_pc(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
    }

    /// The faults (empty for a PC capture).
    pub fn faults(&self) -> impl Iterator<Item = Fault> + 'a {
        let n = if self.kind == KIND_FAULTS { self.entries.len() } else { 0 };
        self.entries[..n].chunks_exact(8).map(|c| {
            decode_fault(
                u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn bytes(b: &Block) -> Vec<u8> {
        b.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn block_fits_and_capacities_are_even() {
        assert_eq!(SIZE, 2048);
        assert_eq!(PC_CAP, 500);
        assert_eq!(FAULT_CAP, 250);
        assert!(HEADER + PC_CAP as usize * PC_WORDS <= WORDS);
        assert!(HEADER + FAULT_CAP as usize * FAULT_WORDS <= WORDS);
    }

    #[test]
    fn fault_entry_round_trips() {
        for &(addr, pc, l, d, us) in &[
            (0x0020_0000u32, 0x0010_0000u32, true, false, 0u32),
            (0x0035_7FFF, 0x002F_FFFE, false, true, 1024 * 0x3FFFF),
            (0x0034_0004, 0x0011_2345, true, true, 5_000_123),
        ] {
            let [a, b] = encode_fault(addr, pc, l, d, us);
            let f = decode_fault(a, b);
            assert_eq!(f, Fault { addr, pc, loaded: l, data: d, t: (us >> 10) & 0x3FFFF });
        }
        // Time wraps at 18 bits and never spills into the flags.
        let [a, b] = encode_fault(0x0020_0000, 0x0010_0000, false, false, 1024 << 18);
        assert_eq!(decode_fault(a, b).t, 0);
        assert_eq!(a & (F_LOADED | F_DATA), 0);
    }

    #[test]
    fn pc_entry_keeps_the_mode_bit() {
        let s = decode_pc(encode_pc(0x0020_1234, true));
        assert_eq!(s, PcSample { pc: 0x0020_1234, handler: true });
        assert!(!decode_pc(encode_pc(0x0010_0101, false)).handler);
    }

    #[test]
    fn spread_capture_thins_uniformly() {
        let mut b = [0xDEAD_BEEFu32; WORDS];
        start(&mut b, KIND_PC, false, 1000, 77);
        assert!(running(&b, KIND_PC));
        assert!(!running(&b, KIND_FAULTS));
        let n = 5 * PC_CAP + 123;
        for i in 0..n {
            push(&mut b, &[i << 1]);
        }
        let shift = b[W_SHIFT];
        // Stride: the smallest power of two for which the events fit.
        assert!(n.div_ceil(1 << shift) <= PC_CAP);
        assert!(n.div_ceil(1 << (shift - 1)) > PC_CAP);
        let raw = bytes(&b);
        let c = Capture::parse(&raw).unwrap();
        assert_eq!(c.seen, n);
        assert_eq!(c.count, n.div_ceil(1 << shift));
        // Entry i is event i << shift.
        for (i, s) in c.pc_samples().enumerate() {
            assert_eq!(s.pc >> 1, (i as u32) << shift, "entry {i}");
        }
    }

    #[test]
    fn spread_capture_of_faults_keeps_pairs_together() {
        let mut b = [0u32; WORDS];
        start(&mut b, KIND_FAULTS, false, 0, 0);
        for i in 0..1000u32 {
            push(
                &mut b,
                &encode_fault(0x0020_0000 + 4 * i, 0x0010_0000 + 2 * i, i % 3 == 0, false, 1024 * i),
            );
        }
        let raw = bytes(&b);
        let c = Capture::parse(&raw).unwrap();
        assert_eq!(c.shift, 2);
        assert_eq!(c.count, 250);
        for (i, f) in c.faults().enumerate() {
            let ev = (i as u32) << 2;
            assert_eq!(f.addr, 0x0020_0000 + 4 * ev);
            assert_eq!(f.pc, 0x0010_0000 + 2 * ev);
            assert_eq!(f.t, ev);
            assert_eq!(f.loaded, ev % 3 == 0);
        }
    }

    #[test]
    fn burst_capture_stops_when_full() {
        let mut b = [0u32; WORDS];
        start(&mut b, KIND_FAULTS, true, 0, 0);
        for i in 0..300u32 {
            push(&mut b, &encode_fault(0x0020_0000 + i, 0x0010_0000, true, false, 0));
        }
        stop(&mut b, 5000);
        let raw = bytes(&b);
        let c = Capture::parse(&raw).unwrap();
        assert!(c.full && c.burst && !c.running);
        assert_eq!((c.count, c.seen, c.shift), (FAULT_CAP, 300, 0));
        assert_eq!(c.faults().last().unwrap().addr, 0x0020_0000 + FAULT_CAP - 1);
        assert_eq!(c.span_us(), 5000);
        // A stopped capture takes nothing (the firmware checks `running`).
        assert!(!running(&b, KIND_FAULTS));
    }

    #[test]
    fn parse_rejects_bad_blocks() {
        let mut b = [0u32; WORDS];
        assert_eq!(Capture::parse(&bytes(&b)).unwrap_err(), ParseError::Magic);
        start(&mut b, KIND_PC, false, 0, 0);
        b[W_COUNT] = 10_000;
        assert_eq!(Capture::parse(&bytes(&b)).unwrap_err(), ParseError::Count(10_000));
        b[W_COUNT] = 0;
        b[W_VERSION] = 9;
        assert_eq!(Capture::parse(&bytes(&b)).unwrap_err(), ParseError::Version(9));
        b[W_VERSION] = VERSION;
        b[W_MODE] = 3;
        assert_eq!(Capture::parse(&bytes(&b)).unwrap_err(), ParseError::Kind(3));
        assert_eq!(Capture::parse(&bytes(&b)[..20]).unwrap_err(), ParseError::Short);
    }
}
