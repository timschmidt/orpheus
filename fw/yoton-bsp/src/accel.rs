//! The motion sensor: a MiraMEMS DA2xx-family 3-axis accelerometer at
//! I2C `0x27` on the software I2C (GPIO24 SCL, GPIO25 SDA), id `0x13`
//! (docs/hw/fm-i2c-imu.md §3, 03-hal-drivers §15, WP SENS-1).
//!
//! What the part is beyond "DA2xx, id 0x13" is not known (U-ACC-1): the
//! stock firmware never checks, and DA213/DA215/DA217/DA226/DA228 share
//! the id and the register map used here. So this driver uses only what
//! the stock firmware uses: no FIFO (whether the fitted part has one is
//! unknown; `acc regs` dumps the map for identification), no interrupt pin
//! (its wiring is unknown), polled burst reads of the six data bytes.
//!
//! **Counting mode** is the vendor's configuration (fm-i2c-imu.md §3.2,
//! steps 1–4, 7–9, 14; the tap-interrupt leftovers 5, 6, 10–13 are left
//! out): ± 2 g, ODR code 6 (62.5 Hz), normal mode. The player samples it at
//! 25 Hz (`firmware::sensor`); the part keeps converting at 62.5 Hz in
//! between, so every read returns a conversion at most 16 ms old. Suspend
//! (`0x11` bits 7:6 ← `10`) whenever nothing counts, and at boot.
//!
//! One bus user: only the sensor task (and the developer commands, which
//! run on the same executor, between its transactions) talk to it. The
//! tuner (FM-1) will need a shared bus.

use atj2157_hal::i2c_soft::{Error, SoftI2c};
use embassy_time::Timer;

use crate::pins;

/// 7-bit address.
pub const ADDR: u8 = 0x27;
/// Register `0x01` of the MiraMEMS family.
pub const CHIP_ID: u8 = 0x13;
/// The ODR register's value in counting mode (vendor: 62.5 Hz).
pub const ODR_COUNTING: u8 = 0x06;
/// Counts per g at ± 2 g (12 bits, right-aligned).
pub const COUNTS_PER_G: i16 = 1024;
/// Half clock period of the bus, µs: 100 kHz, timed on the time base
/// (`atj2157_hal::i2c_soft`). Until AU-PERF (2026-11-25) the same setting
/// gave about 30 kHz (the spin loop ran three times as long), which is the
/// rate this part was used at before; the stock firmware drives it faster
/// still (fm-i2c-imu.md §1.3: delay count 5 against the tuner's 20), and
/// both chips are 400 kHz parts. `acc bus <µs>` changes it until reboot.
pub const HALF_US: u32 = 5;
/// Half periods of one sample (`read`): 181.
pub const SAMPLE_HALVES: u32 = atj2157_hal::i2c_soft::write_read_halves(1, 6);

/// Registers used (names of the DA213 map, fm-i2c-imu.md §3.2).
pub mod reg {
    pub const SPI_CONFIG: u8 = 0x00;
    pub const CHIP_ID: u8 = 0x01;
    pub const ACC_X_LSB: u8 = 0x02;
    pub const RANGE: u8 = 0x0F;
    pub const ODR_AXIS: u8 = 0x10;
    pub const MODE_BW: u8 = 0x11;
    pub const UNLOCK: u8 = 0x7F;
}

/// The vendor's "open" sequence, functional steps only: (register, mask,
/// value), each a read-modify-write. **VERBATIM** (fm-i2c-imu.md §3.2;
/// meanings inferred from the DA213 map: soft reset, ± 2 g, low-power then
/// ODR 62.5 Hz all axes, the three-write unlock, normal mode). The reset
/// is followed by a 10 ms wait.
const OPEN: [(u8, u8, u8); 7] = [
    (reg::RANGE, 0x03, 0x00),
    (reg::MODE_BW, 0xFF, 0x5E),
    (reg::ODR_AXIS, 0xFF, ODR_COUNTING),
    (reg::UNLOCK, 0xFF, 0x83),
    (reg::UNLOCK, 0xFF, 0x69),
    (reg::UNLOCK, 0xFF, 0xBD),
    (reg::MODE_BW, 0xFF, 0x1E),
];

/// The accelerometer.
pub struct Accel {
    bus: SoftI2c,
}

impl Default for Accel {
    fn default() -> Self {
        Self::new()
    }
}

impl Accel {
    /// Configure the bus pins (released, pull-ups on); nothing is sent.
    /// 100 kHz ([`HALF_US`]).
    #[must_use]
    pub fn new() -> Accel {
        Accel { bus: SoftI2c::new(pins::I2C_SCL, pins::I2C_SDA, HALF_US) }
    }

    /// The bus's half clock period, µs (developer `acc bus`).
    pub fn set_bus_half_us(&mut self, us: u32) {
        self.bus.set_half_us(us);
    }

    #[must_use]
    pub fn bus_half_us(&self) -> u32 {
        self.bus.half_us()
    }

    /// Read a register.
    pub fn reg(&self, r: u8) -> Result<u8, Error> {
        let mut b = [0u8; 1];
        self.bus.write_read(ADDR, &[r], &mut b)?;
        Ok(b[0])
    }

    /// Read `out.len()` registers from `first` (the part auto-increments).
    pub fn regs(&self, first: u8, out: &mut [u8]) -> Result<(), Error> {
        self.bus.write_read(ADDR, &[first], out)
    }

    /// The id register (`0x13` for this family).
    pub fn id(&self) -> Result<u8, Error> {
        self.reg(reg::CHIP_ID)
    }

    /// `reg = (reg & !mask) | value`, three attempts (as the vendor).
    fn update(&self, r: u8, mask: u8, value: u8) -> Result<(), Error> {
        let mut last = Err(Error::Nack);
        for _ in 0..3 {
            last = self.reg(r).and_then(|v| self.bus.write(ADDR, &[r, (v & !mask) | (value & mask)]));
            if last.is_ok() {
                break;
            }
        }
        last
    }

    /// Reset and configure for counting; returns the id.
    pub async fn open(&self) -> Result<u8, Error> {
        let (l, s) = self.bus.lines();
        if !(l && s) {
            self.bus.recover();
        }
        self.update(reg::SPI_CONFIG, 0x24, 0x24)?;
        Timer::after_millis(10).await;
        for (r, m, v) in OPEN {
            self.update(r, m, v)?;
        }
        Timer::after_millis(10).await;
        self.id()
    }

    /// Stop converting (`0x11` bits 7:6 ← `10`, the vendor's "close").
    pub fn suspend(&self) -> Result<(), Error> {
        self.update(reg::MODE_BW, 0xC0, 0x80)
    }

    /// One sample: x, y, z in counts (1024 per g), one burst read of
    /// registers `0x02–0x07` (little-endian, left-justified 12 bits).
    pub fn read(&self) -> Result<[i16; 3], Error> {
        let mut b = [0u8; 6];
        self.regs(reg::ACC_X_LSB, &mut b)?;
        Ok([
            i16::from_le_bytes([b[0], b[1]]) >> 4,
            i16::from_le_bytes([b[2], b[3]]) >> 4,
            i16::from_le_bytes([b[4], b[5]]) >> 4,
        ])
    }
}
