//! The RDA5807-family tuner at the register level (fm-i2c-imu.md §2).
//!
//! Sequential access at 7-bit address `0x10`: a write always starts at
//! register 02h (high byte first) and carries as many registers as bytes
//! sent; a read always starts at 0Ah. The driver keeps a shadow of 02h–07h
//! and sends prefixes of it; the one-shot bits (`TUNE`, `SEEK`,
//! `SOFT_RESET`) are added to a write, never kept.
//!
//! Register names follow the RDA5807M datasheet **[inferred]** for this
//! board; every value written is either the vendor's (marked VERBATIM) or
//! a documented field.

use crate::region::Region;

/// 7-bit I2C address of sequential access (bytes `0x20` / `0x21`).
pub const ADDR: u8 = 0x10;
/// 7-bit I2C address of random access (RDA5807M class): a register index
/// byte, then the register's 16 bits. Used only to read the chip id from
/// register 00h when the vendor's sequential method gives none.
pub const ADDR_RANDOM: u8 = 0x11;
/// Register 00h (random access): the chip id.
pub const REG_CHIP_ID: u8 = 0x00;

/// The board around the tuner: the bus (sequential mode at [`ADDR`]) and
/// the analogue route into the headphone amplifier.
pub trait Port {
    /// Send `data` from register 02h on. `false` if the chip did not
    /// acknowledge (the port retries as it sees fit).
    fn write(&mut self, data: &[u8]) -> bool;
    /// Read `out.len()` bytes from register 0Ah on.
    fn read(&mut self, out: &mut [u8]) -> bool;
    /// Random access at [`ADDR_RANDOM`]: send the register index `reg`,
    /// repeated start, read `out.len()` bytes.
    fn read_reg(&mut self, reg: u8, out: &mut [u8]) -> bool;
    /// Switch the FM input of the amplifier mixer, or set its gain.
    fn analog(&mut self, a: Analog);
}

/// The vendor's FM bypass sequence into the amplifier mixer
/// (`AUD_DEV` API 15 type 1, audio.md §4): `Precharge`, 100 ms,
/// `Connect`, 100 ms, `Release`. `Disconnect` is API 16's FM half.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Analog {
    /// `PA_ANTIPOP` bit 21 set (pre-charge / switch mute).
    Precharge,
    /// `PA_ANTIPOP` bits 23:22 = 10b and bit 20 (FM → amplifier) set.
    Connect,
    /// `PA_ANTIPOP` bit 21 cleared.
    Release,
    /// `PA_ANTIPOP` bit 20 cleared: the tuner is no longer heard.
    Disconnect,
    /// Amplifier gain `PA_CTL[14:12]`, 0–7.
    Gain(u8),
}

/// Register 02h bits.
pub mod r02 {
    pub const DHIZ: u16 = 1 << 15;
    /// 1 = audio on (not muted).
    pub const DMUTE: u16 = 1 << 14;
    pub const MONO: u16 = 1 << 13;
    pub const SEEKUP: u16 = 1 << 9;
    pub const SEEK: u16 = 1 << 8;
    /// 1 = a seek stops at the band edge (0 = wraps).
    pub const SKMODE: u16 = 1 << 7;
    pub const RDS_EN: u16 = 1 << 3;
    pub const SOFT_RESET: u16 = 1 << 1;
    pub const ENABLE: u16 = 1;
}

/// Register 03h fields.
pub mod r03 {
    pub const TUNE: u16 = 1 << 4;
    pub const CHAN_SHIFT: u16 = 6;
}

/// Register 04h bits.
pub mod r04 {
    /// De-emphasis 50 µs (0 = 75 µs).
    pub const DE: u16 = 1 << 11;
    pub const SOFTMUTE_EN: u16 = 1 << 9;
}

/// Register 0Ah bits.
pub mod r0a {
    pub const RDSR: u16 = 1 << 15;
    pub const STC: u16 = 1 << 14;
    /// Seek failed (reached the band edge without a station).
    pub const SF: u16 = 1 << 13;
    pub const RDSS: u16 = 1 << 12;
    pub const ST: u16 = 1 << 10;
    pub const READCHAN: u16 = 0x3FF;
}

/// Register 0Bh bits.
pub mod r0b {
    pub const RSSI_SHIFT: u16 = 9;
    pub const FM_TRUE: u16 = 1 << 8;
    pub const FM_READY: u16 = 1 << 7;
}

/// Init table for chip id 0x5804 (registers 02h–35h). VERBATIM
/// (`DRV_FM.DRV` 0x3005EC): RDA's reference table with 02h set for the
/// 24 MHz reference in direct-input mode (`0xC451`).
pub const TABLE_A: [u16; 52] = [
    0xC451, 0x1B90, 0x0400, 0x86AD, 0x8000, 0x5F1A, 0x5EC6, 0x0000, // 02h
    0x406E, 0x2D80, 0x5803, 0x5804, 0x5804, 0x5804, 0x0047, 0x9000, // 0Ah
    0xF587, 0x7F0B, 0x04F1, 0x5EC0, 0x41E0, 0x506F, 0x5592, 0x007D, // 12h
    0x10C0, 0x0780, 0x411D, 0x4006, 0x1F9B, 0x4C2B, 0x8110, 0x45A0, // 1Ah
    0x193F, 0xAF40, 0x0681, 0x1B2A, 0x0D04, 0x809F, 0x178A, 0xD349, // 22h
    0x1142, 0xA0C4, 0x3EBB, 0x0000, 0x5804, 0x5804, 0x5804, 0x0074, // 2Ah
    0x3D00, 0x030C, 0x2F68, 0x3877, // 32h
];

/// Init table for chip id 0x5801 (02h–38h). VERBATIM (`DRV_FM.DRV`
/// 0x300536).
pub const TABLE_B: [u16; 55] = [
    0xC451, 0x0000, 0x0400, 0x86AD, 0x4000, 0x56C6, 0x0000, 0x0000, // 02h
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0006, 0x0019, // 0Ah
    0x2A11, 0x002E, 0x2A30, 0xB83C, 0x9000, 0x2A91, 0x8412, 0x00A8, // 12h
    0xC400, 0xE000, 0x301D, 0x816A, 0x4608, 0x0086, 0x0661, 0x0000, // 1Ah
    0x109E, 0x2040, 0x0408, 0x0608, 0xE105, 0x3B6C, 0x2BEC, 0x090F, // 22h
    0x3414, 0x1450, 0x096D, 0x2D96, 0x01DA, 0x2A7B, 0x0821, 0x13D5, // 2Ah
    0x4891, 0x00BC, 0x0896, 0x153C, 0x0B80, 0x25C7, 0x0000, // 32h
];

/// Init table for any other id (02h–25h). VERBATIM (`DRV_FM.DRV`
/// 0x3005A4).
pub const TABLE_C: [u16; 36] = [
    0xC451, 0x0010, 0x0400, 0xC3AD, 0x6000, 0x7E1E, 0x0000, 0x0000, // 02h
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0019, // 0Ah
    0x2A11, 0xB042, 0x2A11, 0xB831, 0xC000, 0x2A91, 0x9400, 0x00A8, // 12h
    0xC400, 0xF7CF, 0x2ADC, 0x806F, 0x4608, 0x0086, 0x0661, 0x0000, // 1Ah
    0x109E, 0x23C8, 0x0406, 0x0E1C, // 22h
];

/// The chip id this board carries (read on the player, 2026-11-23).
pub const ID_A: u16 = 0x5804;
pub const ID_B: u16 = 0x5801;

/// Which init table the chip got.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Variant {
    /// Not identified yet.
    #[default]
    Unknown,
    /// Id 0x5804 (RDA5807M/MP/FP class, with RDS **[inferred]**).
    A,
    /// Id 0x5801 (RDA5807H class).
    B,
    /// Anything else (the vendor's fallback table).
    C,
}

impl Variant {
    #[must_use]
    pub const fn of(id: u16) -> Variant {
        match id {
            ID_A => Variant::A,
            ID_B => Variant::B,
            _ => Variant::C,
        }
    }

    fn table(self) -> &'static [u16] {
        match self {
            Variant::A | Variant::Unknown => &TABLE_A,
            Variant::B => &TABLE_B,
            Variant::C => &TABLE_C,
        }
    }

    /// RDS is tried on the 0x5804 class only.
    #[must_use]
    pub const fn rds(self) -> bool {
        matches!(self, Variant::A)
    }
}

/// What the tuner is set up for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setup {
    pub region: Region,
    pub khz: u32,
    pub mono: bool,
    pub rds: bool,
    pub softmute: bool,
    /// Seek threshold (05h `SEEKTH`, 0–15; 8 is the vendor's).
    pub threshold: u8,
    /// Tuner volume 0–15.
    pub volume: u8,
}

/// One status read: 0Ah and 0Bh, and with RDS the four blocks (0Ch–0Fh).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    pub r0a: u16,
    pub r0b: u16,
    pub blocks: [u16; 4],
    /// The blocks were read.
    pub with_blocks: bool,
}

impl Status {
    #[must_use]
    pub const fn stc(&self) -> bool {
        self.r0a & r0a::STC != 0
    }
    #[must_use]
    pub const fn seek_failed(&self) -> bool {
        self.r0a & r0a::SF != 0
    }
    #[must_use]
    pub const fn stereo(&self) -> bool {
        self.r0a & r0a::ST != 0
    }
    #[must_use]
    pub const fn rds_ready(&self) -> bool {
        self.with_blocks && self.r0a & r0a::RDSR != 0
    }
    #[must_use]
    pub const fn chan(&self) -> u16 {
        self.r0a & r0a::READCHAN
    }
    /// 0–127 (about 1 dB per step).
    #[must_use]
    pub const fn rssi(&self) -> u8 {
        (self.r0b >> r0b::RSSI_SHIFT) as u8
    }
    #[must_use]
    pub const fn fm_true(&self) -> bool {
        self.r0b & r0b::FM_TRUE != 0
    }
    /// Block errors of A and B (0 none, 1 1–2 corrected, 2 3–5, 3 more).
    #[must_use]
    pub const fn bler(&self) -> (u8, u8) {
        (((self.r0b >> 2) & 3) as u8, (self.r0b & 3) as u8)
    }

    fn from_bytes(b: &[u8]) -> Status {
        let mut w = [0u16; 6];
        for (o, c) in w.iter_mut().zip(b.chunks_exact(2)) {
            *o = u16::from_be_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)]);
        }
        Status { r0a: w[0], r0b: w[1], blocks: [w[2], w[3], w[4], w[5]], with_blocks: b.len() >= 12 }
    }
}

/// Big-endian bytes of `words` into `out`; the number of bytes written.
fn put(out: &mut [u8], words: impl Iterator<Item = u16>) -> usize {
    let mut n = 0;
    for (c, w) in out.chunks_exact_mut(2).zip(words) {
        c.copy_from_slice(&w.to_be_bytes());
        n += 2;
    }
    n
}

/// The register-level driver: a shadow of 02h–07h and the sequences.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tuner {
    regs: [u16; 6],
    variant: Variant,
    region: Region,
}

impl Default for Tuner {
    fn default() -> Self {
        Tuner::new()
    }
}

const R02: usize = 0;
const R03: usize = 1;
const R04: usize = 2;
const R05: usize = 3;

impl Tuner {
    #[must_use]
    pub const fn new() -> Tuner {
        Tuner { regs: [0; 6], variant: Variant::Unknown, region: Region::Europe }
    }

    /// The shadow of 02h–07h.
    #[must_use]
    pub const fn regs(&self) -> [u16; 6] {
        self.regs
    }

    #[must_use]
    pub const fn variant(&self) -> Variant {
        self.variant
    }

    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// Send the first `n` shadow registers with one-shot bits added.
    fn send<P: Port>(&self, p: &mut P, n: usize, add02: u16, add03: u16) -> bool {
        let mut b = [0u8; 12];
        let words = self.regs.iter().enumerate().take(n).map(|(i, &w)| match i {
            R02 => w | add02,
            R03 => w | add03,
            _ => w,
        });
        let len = put(&mut b, words);
        p.write(b.get(..len).unwrap_or(&[]))
    }

    /// Soft reset (02h = `SOFT_RESET`). The chip needs ~10 ms before
    /// [`Tuner::identify`] (vendor wait).
    pub fn soft_reset<P: Port>(&mut self, p: &mut P) -> bool {
        p.write(&r02::SOFT_RESET.to_be_bytes())
    }

    /// The chip id: bytes 8–9 of a 10-byte sequential read after the
    /// reset (the vendor's method, fm-i2c-imu.md §2.1); if that is not an
    /// id the vendor knows, register 00h by random access at 0x11 (where
    /// the bring-up image read `0x5804`, 01-hardware §9). `None` if the
    /// chip does not answer at 0x10.
    pub fn identify<P: Port>(&mut self, p: &mut P) -> Option<u16> {
        let mut b = [0u8; 10];
        if !p.read(&mut b) {
            return None;
        }
        let seq = u16::from_be_bytes([b[8], b[9]]);
        if matches!(seq, ID_A | ID_B) {
            return Some(seq);
        }
        let mut r = [0u8; 2];
        if p.read_reg(REG_CHIP_ID, &mut r) {
            let id = u16::from_be_bytes(r);
            if id >> 8 == 0x58 {
                return Some(id);
            }
        }
        Some(seq)
    }

    /// 03h for `khz` (channel, band, spacing; no `TUNE`).
    fn chan_word(region: Region, khz: u32) -> u16 {
        (region.channel(khz) << r03::CHAN_SHIFT) | (region.band(khz).bits << 2) | region.space_bits()
    }

    /// Load the init table for chip `id` with `s` applied, ENABLE set and
    /// the audio muted (`DMUTE` = 0; [`Tuner::set_mute`] opens it). The
    /// vendor waits 500 ms afterwards before using the chip.
    pub fn load<P: Port>(&mut self, p: &mut P, id: u16, s: &Setup) -> bool {
        self.variant = Variant::of(id);
        self.region = s.region;
        let table = self.variant.table();
        let mut words = [0u16; 55];
        for (o, &w) in words.iter_mut().zip(table) {
            *o = w;
        }
        // 02h: the table's value (DHIZ, 24 MHz direct input, ENABLE),
        // muted, mono and RDS as asked.
        let mut r02v = (words[R02] | r02::ENABLE | r02::DHIZ) & !(r02::DMUTE | r02::MONO | r02::RDS_EN);
        if s.mono {
            r02v |= r02::MONO;
        }
        if s.rds && self.variant.rds() {
            r02v |= r02::RDS_EN;
        }
        words[R02] = r02v;
        words[R03] = Self::chan_word(s.region, s.khz);
        let mut r04v = words[R04] & !(r04::DE | r04::SOFTMUTE_EN);
        if s.region.deemphasis_50us() {
            r04v |= r04::DE;
        }
        if s.softmute {
            r04v |= r04::SOFTMUTE_EN;
        }
        words[R04] = r04v;
        match self.variant {
            Variant::B => {
                // VERBATIM (FMb0 0x10060C): this variant takes the
                // threshold in register 23h.
                let th = u16::from(s.threshold & 0x7F);
                if let Some(w) = words.get_mut(0x23 - 2) {
                    *w = ((th >> 1) & 0x3F) << 8 | (((th & 1) << 7) + th);
                }
                words[R05] = (words[R05] & 0xFF00) | 0x80 | u16::from(s.volume & 15);
            }
            _ => words[R05] = Self::r05(s.threshold, s.volume),
        }
        for (o, w) in self.regs.iter_mut().zip(words) {
            *o = w;
        }
        let mut b = [0u8; 110];
        let len = put(&mut b, words.iter().copied().take(table.len()));
        p.write(b.get(..len).unwrap_or(&[]))
    }

    /// 05h: `INT_MODE`, `SEEKTH` (bits 11:8, 0–15), LNA input LNAP,
    /// `VOLUME`. VERBATIM: after its first volume call the vendor runs with
    /// low byte `0x80 | volume` (fm-i2c-imu.md §2.4).
    const fn r05(threshold: u8, volume: u8) -> u16 {
        0x8000 | ((threshold as u16 & 15) << 8) | 0x80 | (volume as u16 & 15)
    }

    /// Tune to `khz` (in the current region's band): 02h without `SEEK`,
    /// 03h with `TUNE`. Completion: [`Status::stc`].
    pub fn tune<P: Port>(&mut self, p: &mut P, khz: u32) -> bool {
        self.regs[R02] &= !r02::SEEK;
        self.regs[R03] = Self::chan_word(self.region, khz);
        self.send(p, 2, 0, r03::TUNE)
    }

    /// Start a hardware seek from the current channel, stopping at the
    /// band edge (`SKMODE`); completion: [`Status::stc`], failure (edge):
    /// [`Status::seek_failed`].
    pub fn seek<P: Port>(&mut self, p: &mut P, up: bool) -> bool {
        let mut add = r02::SEEK | r02::SKMODE;
        if up {
            add |= r02::SEEKUP;
        }
        self.send(p, 1, add, 0)
    }

    /// Stop a seek (02h without `SEEK`).
    pub fn stop_seek<P: Port>(&mut self, p: &mut P) -> bool {
        self.send(p, 1, 0, 0)
    }

    /// After a seek: the channel it stopped on, so later 03h writes keep it.
    pub fn follow(&mut self, khz: u32) {
        self.regs[R03] = Self::chan_word(self.region, khz);
    }

    /// Audio on or off (02h `DMUTE`).
    pub fn set_mute<P: Port>(&mut self, p: &mut P, muted: bool) -> bool {
        if muted {
            self.regs[R02] &= !r02::DMUTE;
        } else {
            self.regs[R02] |= r02::DMUTE;
        }
        self.send(p, 1, 0, 0)
    }

    /// Forced mono (02h `MONO`).
    pub fn set_mono<P: Port>(&mut self, p: &mut P, mono: bool) -> bool {
        if mono {
            self.regs[R02] |= r02::MONO;
        } else {
            self.regs[R02] &= !r02::MONO;
        }
        self.send(p, 1, 0, 0)
    }

    /// Tuner volume 0–15 and seek threshold (05h); exactly 02h–05h are
    /// sent (the vendor sends a stray ninth byte, fm-i2c-imu.md §2.4).
    pub fn set_volume<P: Port>(&mut self, p: &mut P, volume: u8, threshold: u8) -> bool {
        if self.variant == Variant::B {
            self.regs[R05] = (self.regs[R05] & 0xFFF0) | u16::from(volume & 15);
        } else {
            self.regs[R05] = Self::r05(threshold, volume);
        }
        self.send(p, 4, 0, 0)
    }

    /// A new region: band, spacing (03h) and de-emphasis (04h); then
    /// [`Tuner::tune`] to a channel of it.
    pub fn set_region<P: Port>(&mut self, p: &mut P, region: Region, khz: u32) -> bool {
        self.region = region;
        self.regs[R03] = Self::chan_word(region, khz);
        if region.deemphasis_50us() {
            self.regs[R04] |= r04::DE;
        } else {
            self.regs[R04] &= !r04::DE;
        }
        self.send(p, 3, 0, 0)
    }

    /// Power down (02h without `ENABLE`; the vendor's standby, op 1).
    pub fn standby<P: Port>(&mut self, p: &mut P) -> bool {
        self.regs[R02] &= !(r02::ENABLE | r02::SEEK | r02::DMUTE);
        self.send(p, 1, 0, 0)
    }

    /// Read 0Ah–0Bh, and with `blocks` the RDS blocks 0Ch–0Fh too.
    pub fn status<P: Port>(&self, p: &mut P, blocks: bool) -> Option<Status> {
        let mut b = [0u8; 12];
        let n = if blocks { 12 } else { 4 };
        let buf = b.get_mut(..n)?;
        if !p.read(buf) {
            return None;
        }
        Some(Status::from_bytes(buf))
    }

    /// Frequency of a channel read back (0Ah `READCHAN`) in the band
    /// `near` is in.
    #[must_use]
    pub const fn khz_of_chan(&self, near: u32, chan: u16) -> u32 {
        self.region.khz_of(self.region.band(near), chan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeChip;
    use std::vec::Vec;

    fn setup(region: Region, khz: u32) -> Setup {
        Setup { region, khz, mono: false, rds: true, softmute: true, threshold: 8, volume: 9 }
    }

    #[test]
    fn load_writes_the_vendor_table_with_our_fields() {
        let mut chip = FakeChip::new();
        let mut t = Tuner::new();
        assert!(t.soft_reset(&mut chip));
        assert_eq!(chip.writes.last().map(Vec::as_slice), Some(&[0x00, 0x02][..]));
        let id = t.identify(&mut chip).unwrap();
        assert_eq!(id, ID_A);
        assert!(t.load(&mut chip, id, &setup(Region::Europe, 101_900)));
        let w = chip.writes.last().unwrap();
        assert_eq!(w.len(), 104, "table A, 02h–35h");
        let word = |i: usize| u16::from_be_bytes([w[2 * i], w[2 * i + 1]]);
        // 02h: DHIZ, muted, 24 MHz direct input, RDS, ENABLE.
        assert_eq!(word(0), 0x8000 | 0x0451 | r02::RDS_EN);
        // 03h: 101.9 MHz = channel 149 in 87–108, 100 kHz, no TUNE.
        assert_eq!(word(1), 149 << 6);
        // 04h: 50 µs, soft mute.
        assert_eq!(word(2), 0x0400 | r04::DE | r04::SOFTMUTE_EN);
        // 05h: INT_MODE, threshold 8, LNAP, volume 9.
        assert_eq!(word(3), 0x8889);
        // The rest is the vendor table.
        for (i, &v) in TABLE_A.iter().enumerate().skip(4) {
            assert_eq!(word(i), v, "register {:02X}h", i + 2);
        }
        assert_eq!(t.variant(), Variant::A);
    }

    #[test]
    fn us_region_is_75us_and_world_is_50khz() {
        let mut chip = FakeChip::new();
        let mut t = Tuner::new();
        assert!(t.load(&mut chip, ID_A, &setup(Region::UnitedStates, 87_900)));
        assert_eq!(t.regs()[2] & r04::DE, 0);
        assert!(t.load(&mut chip, ID_A, &setup(Region::World, 87_550)));
        assert_eq!(t.regs()[1], (11 << 6) | 2, "channel 11 at 50 kHz spacing");
        assert!(t.load(&mut chip, ID_A, &setup(Region::Wide, 68_300)));
        assert_eq!(t.regs()[1], (33 << 6) | (3 << 2), "65–76 MHz band");
    }

    #[test]
    fn other_variants_get_their_tables() {
        let mut chip = FakeChip::new();
        let mut t = Tuner::new();
        assert!(t.load(&mut chip, ID_B, &setup(Region::Europe, 90_000)));
        assert_eq!(chip.writes.last().unwrap().len(), 110);
        assert_eq!(t.regs()[0] & r02::RDS_EN, 0, "no RDS on the 0x5801 class");
        let w = chip.writes.last().unwrap();
        // Register 23h carries the threshold (8): high (8 >> 1) & 0x3F, low 0 + 8.
        let i = 0x23 - 2;
        assert_eq!(u16::from_be_bytes([w[2 * i], w[2 * i + 1]]), 0x0408);
        assert!(t.load(&mut chip, 0x1234, &setup(Region::Europe, 90_000)));
        assert_eq!(chip.writes.last().unwrap().len(), 72);
        assert_eq!(t.variant(), Variant::C);
    }

    #[test]
    fn writes_are_prefixes_with_one_shot_bits() {
        let mut chip = FakeChip::new();
        let mut t = Tuner::new();
        t.load(&mut chip, ID_A, &setup(Region::Europe, 87_500));
        assert!(t.tune(&mut chip, 94_700));
        let w = chip.writes.last().unwrap();
        assert_eq!(w.len(), 4);
        assert_eq!(u16::from_be_bytes([w[2], w[3]]), (77 << 6) | r03::TUNE);
        assert_eq!(t.regs()[1] & r03::TUNE, 0, "TUNE is not kept");
        assert!(t.seek(&mut chip, true));
        let w = chip.writes.last().unwrap();
        assert_eq!(w.len(), 2);
        let r = u16::from_be_bytes([w[0], w[1]]);
        assert_ne!(r & r02::SEEK, 0);
        assert_ne!(r & r02::SEEKUP, 0);
        assert_ne!(r & r02::SKMODE, 0);
        assert_eq!(t.regs()[0] & r02::SEEK, 0, "SEEK is not kept");
        assert!(t.set_volume(&mut chip, 15, 8));
        assert_eq!(chip.writes.last().unwrap().len(), 8, "exactly 02h–05h");
        assert!(t.set_mute(&mut chip, false));
        assert_ne!(t.regs()[0] & r02::DMUTE, 0);
        assert!(t.standby(&mut chip));
        assert_eq!(t.regs()[0] & (r02::ENABLE | r02::DMUTE), 0);
        assert!(!chip.enabled());
    }

    #[test]
    fn status_bits() {
        let s = Status::from_bytes(&[0x44, 0x95, 0x5D, 0x80, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(s.stc() && s.stereo() && !s.seek_failed() && !s.rds_ready());
        assert_eq!(s.chan(), 0x95);
        assert_eq!(s.rssi(), 0x2E);
        assert!(s.fm_true());
        assert!(s.with_blocks);
        assert_eq!(Status::from_bytes(&[0xC0, 0, 0, 0x0B]).bler(), (2, 3));
    }

    #[test]
    fn identify_uses_the_sequential_read_at_0x10_first() {
        let mut chip = FakeChip::new();
        let mut t = Tuner::new();
        t.soft_reset(&mut chip);
        assert_eq!(t.identify(&mut chip), Some(ID_A));
        assert_eq!(chip.random_reads, 0, "0x11 is not needed when 0Eh holds the id");
        assert_eq!(chip.read_addrs.last(), Some(&ADDR));
    }

    /// Regression: `fm on` reported `chip=0000` on the player (FM-1,
    /// 2026-11-24, first device run): the driver only read the id from 0Eh
    /// of the sequential read at 0x10, and never register 00h at 0x11.
    #[test]
    fn regress_fm_identify_falls_back_to_register_00_at_0x11() {
        // 0Eh after the reset holds no id (seen as 0x0000 on the player).
        let mut chip = FakeChip::new();
        chip.seq_id = 0;
        let mut t = Tuner::new();
        t.soft_reset(&mut chip);
        assert_eq!(t.identify(&mut chip), Some(ID_A));
        assert_eq!(chip.random_reads, 1);
        assert_eq!(chip.read_addrs.last(), Some(&ADDR_RANDOM));
        // Neither method gives an id: the sequential value (fallback table).
        chip.random_id = 0;
        assert_eq!(t.identify(&mut chip), Some(0));
    }

    #[test]
    fn a_silent_bus_fails_cleanly() {
        let mut chip = FakeChip::new();
        chip.absent = true;
        let mut t = Tuner::new();
        assert!(!t.soft_reset(&mut chip));
        assert_eq!(t.identify(&mut chip), None);
        assert_eq!(t.status(&mut chip, true), None);
    }
}
