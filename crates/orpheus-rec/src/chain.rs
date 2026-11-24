//! The per-block processing between the capture ring and the encoder:
//! low-cut → AGC → meter → voice activation.

use crate::agc::{Agc, AgcConfig};
use crate::meter::{Levels, Meter};
use crate::vox::Vox;

/// Low-cut corner frequency.
pub const LOWCUT_HZ: u32 = 100;

/// First-order high-pass, one state per channel (Q15 coefficient).
#[derive(Clone, Copy, Debug, Default)]
pub struct LowCut {
    a: i32,
    x1: [i32; 2],
    y1: [i32; 2],
}

impl LowCut {
    /// `y[n] = a·(y[n−1] + x[n] − x[n−1])`, `a = 1 / (1 + 2π·fc/rate)`.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let w = (2 * 314_159 * LOWCUT_HZ / 100_000).max(1);
        let a = (i64::from(rate) * 32_768 / (i64::from(rate) + i64::from(w))) as i32;
        LowCut { a, x1: [0; 2], y1: [0; 2] }
    }

    pub fn process(&mut self, block: &mut [i16], channels: u8) {
        let ch = usize::from(channels.clamp(1, 2));
        let a = i64::from(self.a);
        for f in block.chunks_exact_mut(ch) {
            for ((s, x1), y1) in f.iter_mut().zip(self.x1.iter_mut()).zip(self.y1.iter_mut()) {
                let x = i32::from(*s);
                let y = ((a * i64::from(*y1 + x - *x1)) >> 15) as i32;
                *x1 = x;
                *y1 = y;
                *s = y.clamp(-32_768, 32_767) as i16;
            }
        }
    }
}

/// What the chain is set to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChainConfig {
    pub lowcut: bool,
    pub agc: bool,
    /// Voice activation threshold in dBFS (0: off).
    pub vox_db: i16,
}

/// The chain's state.
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    cfg: ChainConfig,
    rate: u32,
    channels: u8,
    lowcut: LowCut,
    agc: Agc,
    meter: Meter,
    vox: Option<Vox>,
}

impl Chain {
    #[must_use]
    pub fn new(cfg: ChainConfig, rate: u32, channels: u8) -> Self {
        Chain {
            cfg,
            rate,
            channels: channels.clamp(1, 2),
            lowcut: LowCut::new(rate),
            agc: Agc::new(AgcConfig::default()),
            meter: Meter::default(),
            vox: (cfg.vox_db != 0).then(|| Vox::new(cfg.vox_db, crate::vox::HANG_MS)),
        }
    }

    #[must_use]
    pub fn config(&self) -> ChainConfig {
        self.cfg
    }

    /// Change the settings while recording (the AGC keeps its gain).
    pub fn set_config(&mut self, cfg: ChainConfig) {
        if cfg.vox_db != self.cfg.vox_db {
            self.vox = (cfg.vox_db != 0).then(|| Vox::new(cfg.vox_db, crate::vox::HANG_MS));
        }
        self.cfg = cfg;
    }

    /// Process one block in place; returns whether it is to be recorded
    /// (voice activation; always true without it).
    pub fn process(&mut self, block: &mut [i16]) -> bool {
        let frames = block.len() / usize::from(self.channels);
        let ms = (frames as u64 * 1000 / u64::from(self.rate.max(1))) as u32;
        if self.cfg.lowcut {
            self.lowcut.process(block, self.channels);
        }
        // Voice activation listens before the AGC: it would lift room
        // noise over any threshold.
        let peak_in = block.iter().map(|s| u32::from(s.unsigned_abs())).max().unwrap_or(0);
        if self.cfg.agc {
            self.agc.process(block, frames, self.rate);
        }
        self.meter.feed(block, self.channels, ms.max(1));
        match &mut self.vox {
            Some(v) => v.feed(peak_in, ms.max(1)),
            None => true,
        }
    }

    #[must_use]
    pub fn levels(&self) -> Levels {
        self.meter.levels()
    }

    /// The AGC's gain in tenths of a dB (0 without AGC).
    #[must_use]
    pub fn agc_db10(&self) -> i16 {
        if self.cfg.agc { self.agc.gain_db10() } else { 0 }
    }

    /// Voice activation is on and the gate is closed.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.vox.is_some_and(|v| !v.is_open())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeAdc, Signal};

    #[test]
    fn lowcut_removes_dc_and_hum_keeps_speech() {
        let mut lc = LowCut::new(16_000);
        let mut dc = [8_000i16; 1600];
        lc.process(&mut dc, 1);
        assert!(dc[1599].abs() < 100, "{}", dc[1599]);
        let mut adc = FakeAdc::new(16_000, 1, Signal::Tone { hz: 1000, amp: 10_000 }, 1);
        let mut b = [0i16; 1600];
        adc.fill(&mut b);
        lc.process(&mut b, 1);
        let peak = b[800..].iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 9_500, "{peak}");
    }

    #[test]
    fn vox_gates_blocks() {
        let cfg = ChainConfig { vox_db: -40, ..ChainConfig::default() };
        let mut c = Chain::new(cfg, 16_000, 1);
        let mut quiet = [3i16; 320];
        assert!(!c.process(&mut quiet));
        assert!(c.waiting());
        let mut loud = [3_000i16; 320];
        assert!(c.process(&mut loud));
        assert!(!c.waiting());
    }
}
