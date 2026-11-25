//! Software I2C master on two GPIOs (03-hal-drivers §15; fm-i2c-imu.md §1).
//!
//! The stock firmware bit-bangs the bus push-pull. This driver emulates open
//! drain instead (drive low = output enabled with latch 0; release = output
//! disabled) with the pad's 10 kΩ pull-up on as a backstop, samples SCL for
//! clock stretching, and recovers a stuck bus with nine clocks. Default
//! speed 100 kHz. The hardware TWI1 on the same pads is a later step
//! (U-FM-1).
//!
//! **Bit timing** (AU-PERF, 2026-11-25): each half clock period is timed on
//! the time base (`delay::wait_us`, at least `half_us`). It used to be
//! `delay::spin_us`, which spins about three times as long as asked: the
//! "100 kHz" bus ran at roughly 30 kHz, and the step counter's 25 samples
//! a second (181 half periods each) took about 8 % of the CPU in the T-8
//! PC profile. Time in interrupts now counts towards a half period instead
//! of adding to it (the master drives the clock: a longer half is
//! harmless).

use atj2157_pac::gpio::{self, ctl};

use crate::delay::wait_us;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Address not acknowledged.
    Nack,
    /// Data byte not acknowledged.
    DataNack,
    /// SDA or SCL held low.
    BusStuck,
}

pub struct SoftI2c {
    scl: u8,
    sda: u8,
    half_us: u32,
}

/// Longest clock stretch accepted (µs).
const STRETCH_US: u32 = 1000;

/// Half clock periods one [`SoftI2c::write_read`] of `w` then `r` bytes
/// takes: start (3), each byte with its acknowledge (19), the repeated
/// start (1 + 3), stop (3). The accelerometer's sample (one register
/// byte, six data bytes) is 181: 0.9 ms of bus at 100 kHz.
#[must_use]
pub const fn write_read_halves(w: u32, r: u32) -> u32 {
    3 + 19 * (1 + w) + 4 + 19 * (1 + r) + 3
}

fn bank_bit(pin: u8) -> (usize, u32) {
    (usize::from(pin / 32), 1 << (pin % 32))
}

impl SoftI2c {
    /// Configure the pins (GPIO function, input enabled, latch low, pull-up,
    /// released). `half_us` = half clock period in µs (5 → 100 kHz).
    #[must_use]
    pub fn new(scl: u8, sda: u8, half_us: u32) -> Self {
        for p in [scl, sda] {
            let (b, m) = bank_bit(p);
            gpio::brr(b).write(m);
            gpio::mfp(usize::from(p)).modify(|v| {
                (v & !(ctl::MFP.mask() | ctl::GPIOOUTEN | ctl::PULL_MASK)) | ctl::GPIOINEN | ctl::GPIO10KPUEN
            });
        }
        Self { scl, sda, half_us }
    }

    fn drive_low(&self, pin: u8) {
        gpio::mfp(usize::from(pin)).set_bits(ctl::GPIOOUTEN);
    }
    fn release(&self, pin: u8) {
        gpio::mfp(usize::from(pin)).clear_bits(ctl::GPIOOUTEN);
    }
    fn level(&self, pin: u8) -> bool {
        let (b, m) = bank_bit(pin);
        gpio::idat(b).read() & m != 0
    }
    fn d(&self) {
        wait_us(self.half_us);
    }

    /// The half clock period in µs (5: 100 kHz).
    pub fn set_half_us(&mut self, us: u32) {
        self.half_us = us.clamp(1, 1000);
    }

    #[must_use]
    pub fn half_us(&self) -> u32 {
        self.half_us
    }

    fn scl_high(&self) -> Result<(), Error> {
        self.release(self.scl);
        // Clock stretching: wait up to ~1 ms for SCL to rise.
        let t0 = crate::time::now_us_raw();
        for _ in 0..1u32 << 16 {
            if self.level(self.scl) {
                return Ok(());
            }
            if crate::time::now_us_raw().wrapping_sub(t0) > STRETCH_US {
                break;
            }
        }
        Err(Error::BusStuck)
    }

    /// Nine clocks with SDA released, then STOP (bus recovery).
    pub fn recover(&self) {
        self.release(self.sda);
        for _ in 0..9 {
            self.drive_low(self.scl);
            self.d();
            let _ = self.scl_high();
            self.d();
            if self.level(self.sda) {
                break;
            }
        }
        let _ = self.stop();
    }

    fn start(&self) -> Result<(), Error> {
        self.release(self.sda);
        self.scl_high()?;
        self.d();
        if !self.level(self.sda) {
            return Err(Error::BusStuck);
        }
        self.drive_low(self.sda);
        self.d();
        self.drive_low(self.scl);
        self.d();
        Ok(())
    }

    fn stop(&self) -> Result<(), Error> {
        self.drive_low(self.sda);
        self.d();
        self.scl_high()?;
        self.d();
        self.release(self.sda);
        self.d();
        Ok(())
    }

    fn write_byte(&self, b: u8) -> Result<bool, Error> {
        for i in (0..8).rev() {
            if b >> i & 1 != 0 {
                self.release(self.sda);
            } else {
                self.drive_low(self.sda);
            }
            self.d();
            self.scl_high()?;
            self.d();
            self.drive_low(self.scl);
        }
        self.release(self.sda);
        self.d();
        self.scl_high()?;
        self.d();
        let ack = !self.level(self.sda);
        self.drive_low(self.scl);
        self.d();
        Ok(ack)
    }

    fn read_byte(&self, ack: bool) -> Result<u8, Error> {
        self.release(self.sda);
        let mut v = 0u8;
        for _ in 0..8 {
            self.d();
            self.scl_high()?;
            self.d();
            v = v << 1 | u8::from(self.level(self.sda));
            self.drive_low(self.scl);
        }
        if ack {
            self.drive_low(self.sda);
        } else {
            self.release(self.sda);
        }
        self.d();
        self.scl_high()?;
        self.d();
        self.drive_low(self.scl);
        self.release(self.sda);
        self.d();
        Ok(v)
    }

    fn finish<T>(&self, r: Result<T, Error>) -> Result<T, Error> {
        let s = self.stop();
        // Leave both lines released (idle high).
        self.release(self.scl);
        self.release(self.sda);
        match (r, s) {
            (Err(e), _) | (Ok(_), Err(e)) => Err(e),
            (Ok(v), Ok(())) => Ok(v),
        }
    }

    /// Is there a device at 7-bit `addr`?
    pub fn probe(&self, addr: u8) -> Result<bool, Error> {
        let r = self.start().and_then(|()| self.write_byte(addr << 1));
        self.finish(r)
    }

    pub fn write(&self, addr: u8, data: &[u8]) -> Result<(), Error> {
        let r = (|| {
            self.start()?;
            if !self.write_byte(addr << 1)? {
                return Err(Error::Nack);
            }
            for &b in data {
                if !self.write_byte(b)? {
                    return Err(Error::DataNack);
                }
            }
            Ok(())
        })();
        self.finish(r)
    }

    pub fn read(&self, addr: u8, out: &mut [u8]) -> Result<(), Error> {
        let r = (|| {
            self.start()?;
            if !self.write_byte(addr << 1 | 1)? {
                return Err(Error::Nack);
            }
            let n = out.len();
            for (i, o) in out.iter_mut().enumerate() {
                *o = self.read_byte(i + 1 < n)?;
            }
            Ok(())
        })();
        self.finish(r)
    }

    /// Write `w`, repeated start, read into `out`.
    pub fn write_read(&self, addr: u8, w: &[u8], out: &mut [u8]) -> Result<(), Error> {
        let r = (|| {
            self.start()?;
            if !self.write_byte(addr << 1)? {
                return Err(Error::Nack);
            }
            for &b in w {
                if !self.write_byte(b)? {
                    return Err(Error::DataNack);
                }
            }
            // Repeated start.
            self.release(self.sda);
            self.d();
            self.start()?;
            if !self.write_byte(addr << 1 | 1)? {
                return Err(Error::Nack);
            }
            let n = out.len();
            for (i, o) in out.iter_mut().enumerate() {
                *o = self.read_byte(i + 1 < n)?;
            }
            Ok(())
        })();
        self.finish(r)
    }

    /// Line levels (SCL, SDA) while released.
    #[must_use]
    pub fn lines(&self) -> (bool, bool) {
        self.release(self.scl);
        self.release(self.sda);
        wait_us(10);
        (self.level(self.scl), self.level(self.sda))
    }
}
