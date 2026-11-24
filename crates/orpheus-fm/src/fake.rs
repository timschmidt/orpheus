//! A register-level fake RDA5807 (feature `fake`; always in this crate's
//! tests) with a set of stations: RSSI, stereo, RDS service name and radio
//! text. It decodes the writes the driver sends (sequential from 02h),
//! answers reads from 0Ah, and runs tunes and hardware seeks over a number
//! of status reads instead of time. The simulator's radio uses it.

use std::vec::Vec;

use crate::chip::{Analog, Port, r02, r03, r04};
use crate::rds::{PS_LEN, RT_LEN, group_0a, group_2a};

/// A station of the fake's world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FakeStation {
    pub khz: u32,
    pub rssi: u8,
    pub stereo: bool,
    /// RDS service name and radio text (`""` = none).
    pub ps: &'static str,
    pub rt: &'static str,
}

const fn st(khz: u32, rssi: u8, stereo: bool, ps: &'static str, rt: &'static str) -> FakeStation {
    FakeStation { khz, rssi, stereo, ps, rt }
}

/// The world: odd tenths (so the United States region reaches them), the
/// 24 MHz harmonic at 96.0 MHz, a station too weak for the default seek
/// threshold, one in Japan's band and one in 65–76 MHz.
pub const STATIONS: [FakeStation; 10] = [
    st(68_300, 35, false, "", ""),
    st(77_100, 44, true, "J-WAVE", "Tokyo Hot 100"),
    st(88_300, 34, false, "", ""),
    st(91_500, 45, true, "KLASSIK", "Mozart: Symphony No. 40 in G minor"),
    st(94_700, 52, true, "ROCK FM", "Now: Queen - Don't Stop Me Now"),
    st(96_000, 40, false, "", ""),
    st(99_900, 38, true, "RADIO 1", ""),
    st(101_900, 60, true, "PARADISE", "Radio Paradise - eclectic music"),
    st(104_300, 33, true, "NEWS 24", "Traffic: A9 northbound closed"),
    st(106_700, 25, false, "", ""),
];

/// RSSI between stations.
const NOISE: u8 = 10;

pub struct FakeChip {
    pub stations: Vec<FakeStation>,
    /// Nothing answers on the bus.
    pub absent: bool,
    /// What registers 0Dh–0Fh hold when they carry no RDS (bytes 8–9 of
    /// the vendor's id read are 0Eh), and what register 00h holds (random
    /// access at 0x11).
    pub seq_id: u16,
    pub random_id: u16,
    /// Random-access reads, and the address of every read in order.
    pub random_reads: u32,
    pub read_addrs: Vec<u8>,
    /// Every write, as sent (the oldest are dropped past 512).
    pub writes: Vec<Vec<u8>>,
    /// Every analogue call (the oldest are dropped past 512).
    pub analog: Vec<Analog>,
    /// Status reads a tune / a hardware seek takes.
    pub tune_reads: u32,
    pub seek_reads: u32,
    regs: [u16; 0x40],
    khz: u32,
    busy: u32,
    seek: Option<(u32, bool)>,
    stc: bool,
    sf: bool,
    rds_n: u32,
    reads: u32,
    routed: bool,
    gain: u8,
}

impl Default for FakeChip {
    fn default() -> Self {
        FakeChip::new()
    }
}

impl FakeChip {
    #[must_use]
    pub fn new() -> FakeChip {
        FakeChip {
            stations: STATIONS.to_vec(),
            absent: false,
            seq_id: crate::chip::ID_A,
            random_id: crate::chip::ID_A,
            random_reads: 0,
            read_addrs: Vec::new(),
            writes: Vec::new(),
            analog: Vec::new(),
            tune_reads: 2,
            seek_reads: 3,
            regs: [0; 0x40],
            khz: 87_000,
            busy: 0,
            seek: None,
            stc: false,
            sf: false,
            rds_n: 0,
            reads: 0,
            routed: false,
            gain: 7,
        }
    }

    /// The frequency the chip is on.
    #[must_use]
    pub fn khz(&self) -> u32 {
        self.khz
    }
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.regs[2] & r02::ENABLE != 0
    }
    /// Enabled, outputs on and not muted.
    #[must_use]
    pub fn audible(&self) -> bool {
        self.enabled() && self.regs[2] & r02::DMUTE != 0 && self.regs[2] & r02::DHIZ != 0
    }
    #[must_use]
    pub fn mono(&self) -> bool {
        self.regs[2] & r02::MONO != 0
    }
    #[must_use]
    pub fn de_50us(&self) -> bool {
        self.regs[4] & r04::DE != 0
    }
    #[must_use]
    pub fn volume(&self) -> u8 {
        (self.regs[5] & 15) as u8
    }
    #[must_use]
    pub fn routed(&self) -> bool {
        self.routed
    }
    #[must_use]
    pub fn gain(&self) -> u8 {
        self.gain
    }
    /// As a tuner left running by a reset of the player alone.
    pub fn force_enabled(&mut self) {
        self.regs[2] = r02::ENABLE | r02::DHIZ | r02::DMUTE;
    }

    fn band(&self) -> (u32, u32) {
        match (self.regs[3] >> 2) & 3 {
            0 => (87_000, 108_000),
            1 => (76_000, 91_000),
            2 => (76_000, 108_000),
            _ => (65_000, 76_000),
        }
    }

    fn spacing(&self) -> u32 {
        match self.regs[3] & 3 {
            0 => 100,
            1 => 200,
            2 => 50,
            _ => 25,
        }
    }

    fn station(&self) -> Option<&FakeStation> {
        if !self.enabled() {
            return None;
        }
        self.stations.iter().find(|s| s.khz == self.khz)
    }

    fn threshold_rssi(&self) -> u8 {
        (((self.regs[5] >> 8) & 15) * 4) as u8
    }

    /// Where a hardware seek from the current frequency stops, and whether
    /// it failed (band edge, `SKMODE`).
    fn seek_result(&self, up: bool) -> (u32, bool) {
        let (lo, hi) = self.band();
        let sp = self.spacing();
        let th = self.threshold_rssi();
        let mut k = self.khz;
        loop {
            if up {
                if k + sp > hi {
                    return (hi, true);
                }
                k += sp;
            } else {
                if k < lo + sp {
                    return (lo, true);
                }
                k -= sp;
            }
            if self.stations.iter().any(|s| s.khz == k && s.rssi >= th) {
                return (k, false);
            }
        }
    }

    fn reset(&mut self) {
        self.regs = [0; 0x40];
        self.seek = None;
        self.busy = 0;
        self.stc = false;
        self.sf = false;
    }

    fn log<T>(v: &mut Vec<T>, x: T) {
        if v.len() >= 512 {
            v.drain(..256);
        }
        v.push(x);
    }

    fn group(&self, s: &FakeStation) -> [u16; 4] {
        let pi = 0xC000 | (s.khz / 100) as u16;
        let mut ps = [b' '; PS_LEN];
        for (o, c) in ps.iter_mut().zip(s.ps.bytes()) {
            *o = c;
        }
        let mut rt = [b' '; RT_LEN];
        for (o, c) in rt.iter_mut().zip(s.rt.bytes()) {
            *o = c;
        }
        if let Some(end) = rt.get_mut(s.rt.len()) {
            *end = 0x0D;
        }
        let segs = s.rt.len().div_ceil(4).clamp(1, 16) as u32;
        let n = self.rds_n;
        if n % 2 == 0 || s.rt.is_empty() {
            group_0a(pi, ((n / 2) % 4) as u8, &ps)
        } else {
            group_2a(pi, ((n / 2) % segs) as u8, false, &rt)
        }
    }
}

impl Port for FakeChip {
    fn write(&mut self, data: &[u8]) -> bool {
        if self.absent {
            return false;
        }
        Self::log(&mut self.writes, data.to_vec());
        let n = data.len() / 2;
        for (i, c) in data.chunks_exact(2).enumerate() {
            if let Some(r) = self.regs.get_mut(2 + i) {
                *r = u16::from_be_bytes([c[0], c[1]]);
            }
        }
        if n == 0 {
            return true;
        }
        let r2 = self.regs[2];
        if r2 & r02::SOFT_RESET != 0 {
            self.reset();
            return true;
        }
        if n >= 2 && self.regs[3] & r03::TUNE != 0 {
            let (lo, _) = self.band();
            self.khz = lo + u32::from(self.regs[3] >> 6) * self.spacing();
            self.regs[3] &= !r03::TUNE;
            self.seek = None;
            self.busy = self.tune_reads;
            self.stc = false;
            self.sf = false;
            self.rds_n = 0;
        } else if r2 & r02::SEEK != 0 && self.enabled() {
            let up = r2 & r02::SEEKUP != 0;
            self.seek = Some(self.seek_result(up));
            self.regs[2] &= !r02::SEEK;
            self.busy = self.seek_reads;
            self.stc = false;
            self.sf = false;
        } else if self.seek.take().is_some() {
            // SEEK cleared while seeking: stopped where it was.
            self.busy = 0;
        }
        true
    }

    fn read_reg(&mut self, reg: u8, out: &mut [u8]) -> bool {
        if self.absent {
            return false;
        }
        self.random_reads += 1;
        Self::log(&mut self.read_addrs, crate::chip::ADDR_RANDOM);
        let v = if reg == crate::chip::REG_CHIP_ID { self.random_id } else { 0 };
        for (o, b) in out.iter_mut().zip(v.to_be_bytes()) {
            *o = b;
        }
        true
    }

    fn read(&mut self, out: &mut [u8]) -> bool {
        if self.absent {
            return false;
        }
        Self::log(&mut self.read_addrs, crate::chip::ADDR);
        self.reads += 1;
        if self.busy > 0 {
            self.busy -= 1;
            if self.busy == 0 {
                self.stc = true;
                if let Some((k, failed)) = self.seek.take() {
                    self.khz = k;
                    self.sf = failed;
                    self.rds_n = 0;
                }
            }
        }
        let (lo, _) = self.band();
        let chan = (self.khz.saturating_sub(lo) / self.spacing()) as u16 & 0x3FF;
        let station = self.station().copied();
        let tuned = self.busy == 0 && self.seek.is_none();
        let mut r0a = chan;
        if self.stc {
            r0a |= 1 << 14;
        }
        if self.sf {
            r0a |= 1 << 13;
        }
        let mut blocks = [0x5803, self.seq_id, self.seq_id, self.seq_id];
        let rssi = station.map_or(NOISE, |s| s.rssi);
        let mut r0b = u16::from(rssi) << 9 | 1 << 7;
        if let Some(s) = station.filter(|_| tuned) {
            r0b |= 1 << 8;
            if s.stereo && !self.mono() {
                r0a |= 1 << 10;
            }
            let rds_on = self.regs[2] & r02::RDS_EN != 0 && !s.ps.is_empty();
            if rds_on {
                r0a |= 1 << 12;
                if self.reads % 2 == 0 {
                    blocks = self.group(&s);
                    self.rds_n += 1;
                    r0a |= 1 << 15;
                }
            }
        }
        let words = [r0a, r0b, blocks[0], blocks[1], blocks[2], blocks[3]];
        for (c, w) in out.chunks_exact_mut(2).zip(words) {
            c.copy_from_slice(&w.to_be_bytes());
        }
        true
    }

    fn analog(&mut self, a: Analog) {
        match a {
            Analog::Connect => self.routed = true,
            Analog::Disconnect => self.routed = false,
            Analog::Gain(g) => self.gain = g,
            Analog::Precharge | Analog::Release => {}
        }
        Self::log(&mut self.analog, a);
    }
}
