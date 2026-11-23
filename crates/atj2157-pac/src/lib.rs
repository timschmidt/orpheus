//! Register definitions for the Actions ATJ2157.
//!
//! Hand-written, one module per peripheral block. Every register and field
//! carries a **trust tag** in its doc comment:
//!
//! * **\[DS\]** — ATJ2157 Datasheet V1.2 (`docs/vendor/atj2157-datasheet-v1.2.txt`),
//!   with the section number;
//! * **\[HW\]** — verified on the player (by the Rockbox port or a yotonrs
//!   bring-up image);
//! * **\[code\]** — seen in vendor or port code, not verified by us;
//! * **\[inferred\]** — deduced; treat as a hypothesis.
//!
//! Access model: a [`Reg`] is a fixed MMIO address with volatile `read`,
//! `write` and `modify`. Fields are [`Field`] constants (shift + width) or
//! plain bit masks (`u32` constants). [`RoReg`] is a read-only register for
//! blocks yotonrs must never write (the eFuse controller). Vendor and port
//! values are named `VENDOR_*` / `PORT_*` constants with their provenance;
//! the crate contains no driver code. The crate has no notion of ownership;
//! the HAL (`fw/atj2157-hal`) decides who may touch which block.
//!
//! On the host the crate compiles (its constants are unit-tested), but
//! calling `read`/`write` there is undefined: only the device runs them.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod adc;
pub mod cmu;
pub mod dac;
pub mod dma;
pub mod dsp;
pub mod efuse;
pub mod gpio;
pub mod i2s;
pub mod irq;
pub mod lcm;
pub mod memctl;
pub mod pmu;
pub mod pwm;
pub mod rmu;
pub mod rtc;
pub mod sd;
pub mod spi;
pub mod spicache;
pub mod twi;
pub mod uart;
pub mod usb;

/// A 32-bit memory-mapped register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Reg(usize);

impl Reg {
    #[must_use]
    pub const fn at(addr: usize) -> Self {
        Self(addr)
    }

    #[must_use]
    pub const fn addr(self) -> usize {
        self.0
    }

    /// The register `n` words further on (for register arrays).
    #[must_use]
    pub const fn offset(self, bytes: usize) -> Self {
        Self(self.0 + bytes)
    }

    #[inline(always)]
    #[must_use]
    pub fn read(self) -> u32 {
        // SAFETY: `self.0` is a peripheral register address of this SoC
        // (constructed only from the constants in this crate); volatile
        // access has no memory-safety implications for Rust objects.
        unsafe { core::ptr::read_volatile(self.0 as *const u32) }
    }

    #[inline(always)]
    pub fn write(self, v: u32) {
        // SAFETY: as in `read`.
        unsafe { core::ptr::write_volatile(self.0 as *mut u32, v) }
    }

    /// Read-modify-write. Not atomic: callers serialise access (one owner
    /// per register, or a critical section).
    #[inline(always)]
    pub fn modify(self, f: impl FnOnce(u32) -> u32) {
        self.write(f(self.read()));
    }

    #[inline(always)]
    pub fn set_bits(self, mask: u32) {
        self.modify(|v| v | mask);
    }

    #[inline(always)]
    pub fn clear_bits(self, mask: u32) {
        self.modify(|v| v & !mask);
    }

    #[inline(always)]
    #[must_use]
    pub fn get(self, f: Field) -> u32 {
        f.get(self.read())
    }

    #[inline(always)]
    pub fn set(self, f: Field, v: u32) {
        self.modify(|r| f.set(r, v));
    }
}

/// A 32-bit memory-mapped register that this crate only lets you read (used
/// for blocks yotonrs must never write, such as the eFuse controller).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct RoReg(usize);

impl RoReg {
    #[must_use]
    pub const fn at(addr: usize) -> Self {
        Self(addr)
    }

    #[must_use]
    pub const fn addr(self) -> usize {
        self.0
    }

    #[inline(always)]
    #[must_use]
    pub fn read(self) -> u32 {
        // SAFETY: as in `Reg::read`.
        unsafe { core::ptr::read_volatile(self.0 as *const u32) }
    }

    #[inline(always)]
    #[must_use]
    pub fn get(self, f: Field) -> u32 {
        f.get(self.read())
    }
}

/// A bit field: `width` bits starting at `shift`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub shift: u8,
    pub width: u8,
}

impl Field {
    #[must_use]
    pub const fn new(shift: u8, width: u8) -> Self {
        assert!(width > 0 && shift as u32 + width as u32 <= 32);
        Self { shift, width }
    }

    /// Field covering bits `hi..=lo` as printed in the datasheet (`[hi:lo]`).
    #[must_use]
    pub const fn bits(hi: u8, lo: u8) -> Self {
        Self::new(lo, hi - lo + 1)
    }

    #[must_use]
    pub const fn mask(self) -> u32 {
        (u32::MAX >> (32 - self.width as u32)) << self.shift
    }

    #[must_use]
    pub const fn get(self, reg: u32) -> u32 {
        (reg & self.mask()) >> self.shift
    }

    #[must_use]
    pub const fn set(self, reg: u32, v: u32) -> u32 {
        (reg & !self.mask()) | ((v << self.shift) & self.mask())
    }

    /// The value placed in position (for building whole register words).
    #[must_use]
    pub const fn val(self, v: u32) -> u32 {
        (v << self.shift) & self.mask()
    }
}

/// Bit `n` as a mask.
#[must_use]
pub const fn bit(n: u8) -> u32 {
    1 << n
}

/// Test helpers shared by the module tests.
#[cfg(test)]
pub(crate) mod testutil {
    /// Panics if any two masks share a bit (fields of one register that must
    /// not overlap).
    #[track_caller]
    pub fn assert_disjoint(masks: &[u32]) {
        let mut seen = 0u32;
        for (i, &m) in masks.iter().enumerate() {
            assert!(m != 0, "mask #{i} is empty");
            assert!(seen & m == 0, "mask #{i} ({m:#010x}) overlaps earlier masks ({seen:#010x})");
            seen |= m;
        }
    }

    /// Panics if the register addresses are not strictly increasing (a
    /// register list copied from the datasheet in offset order).
    #[track_caller]
    pub fn assert_increasing(addrs: &[usize]) {
        for w in addrs.windows(2) {
            assert!(w[0] < w[1], "{:#x} is not below {:#x}", w[0], w[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_helper() {
        testutil::assert_disjoint(&[Field::bits(3, 0).mask(), bit(4), Field::bits(31, 5).mask()]);
        let r = std::panic::catch_unwind(|| testutil::assert_disjoint(&[bit(3), Field::bits(3, 2).mask()]));
        assert!(r.is_err());
    }

    #[test]
    fn fields() {
        let f = Field::bits(3, 1);
        assert_eq!(f.mask(), 0b1110);
        assert_eq!(f.set(0xFFFF_FFFF, 0), 0xFFFF_FFF1);
        assert_eq!(f.get(0b1010), 0b101);
        assert_eq!(f.val(7), 0b1110);
        assert_eq!(Field::new(0, 32).mask(), u32::MAX);
        assert_eq!(Field::bits(31, 31).mask(), 0x8000_0000);
    }
}
