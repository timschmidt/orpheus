//! DSP accelerator ("FFT/FIR/IIR hardware engine"), `0xC0200000`.
//!
//! The datasheet lists the engine in its feature list only; the memory map
//! (§3.4) prints `0xC0200000` as "-". Everything here is \[code\] from the
//! vendor audio driver `AUD_DEV` (audio.md §7.1), which no module of the stock
//! firmware calls: the block may be unfinished or absent (U-DSP-1).
//!
//! Set-up (vendor, \[code\]): `CLKEN0 |= DMA`, pulse `RST0` bit 0;
//! `CLKEN1 |= DSP`, pulse `RST1.DSP`; [`crate::cmu::DSPCLK`] =
//! [`crate::cmu::VENDOR_DSPCLK`]. Data moves by DMA with device selector
//! `0x1D` ([`VENDOR_DMA_MODE_WRITE`] / [`VENDOR_DMA_MODE_READ`]); the DMA
//! address on the accelerator side is 0.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC020_0000;

/// Main control. \[code\]
pub const CTL: Reg = Reg::at(BASE);
pub mod ctl {
    use super::{Field, bit};
    /// Function: 0 FFT, 1 IFFT, 2 IIR (biquad cascade), 3 "function 3".
    pub const FUNC: Field = Field::bits(1, 0);
    pub const FUNC_FFT: u32 = 0;
    pub const FUNC_IFFT: u32 = 1;
    pub const FUNC_IIR: u32 = 2;
    pub const FUNC_3: u32 = 3;
    /// Written as max(1, function); cleared after a job.
    pub const UNK_3_2: Field = Field::bits(3, 2);
    /// Always written as 1.
    pub const UNK_B30: u32 = bit(30);
    /// Done flag; write 1 to clear.
    pub const DONE: u32 = bit(31);
    /// Values the vendor writes for FFT, IFFT, IIR, function 3.
    pub const VENDOR_FFT: u32 = 0xC000_0004;
    pub const VENDOR_IFFT: u32 = 0xC000_0005;
    pub const VENDOR_IIR: u32 = 0xC000_000A;
    pub const VENDOR_F3: u32 = 0xC000_000F;
}

/// FFT control: `(size_idx << 4) | (b << 2) | (a << 1) | 1`; written 0 first;
/// bit 0 reads 1 while busy. \[code\]
pub const FFT_CTL: Reg = Reg::at(BASE + 0x04);
pub mod fft_ctl {
    use super::{Field, bit};
    pub const START: u32 = bit(0);
    pub const UNK_A: u32 = bit(1);
    pub const UNK_B: u32 = bit(2);
    /// 0 = 128, 1 = 256, 2 = 512 points.
    pub const SIZE: Field = Field::bits(5, 4);
}

/// IIR control: `0xE000_0000 | samples << 16 | sections << 8 | flag << 1 | 1`;
/// bits 2–5 step through the four DMA phases; bit 31 done. Field widths
/// \[inferred\]. \[code\]
pub const IIR_CTL: Reg = Reg::at(BASE + 0x08);
pub mod iir_ctl {
    use super::{Field, bit};
    pub const START: u32 = bit(0);
    pub const FLAG: u32 = bit(1);
    pub const PHASE: Field = Field::bits(5, 2);
    pub const SECTIONS: Field = Field::bits(15, 8);
    pub const SAMPLES: Field = Field::bits(28, 16);
    pub const DONE: u32 = bit(31);
}

/// Function-3 control: `0xC000_0001 | mode << 8`; bits 4–7 step through the
/// DMA phases; bit 31 done. \[code\]
pub const F3_CTL: Reg = Reg::at(BASE + 0x0C);
pub mod f3_ctl {
    use super::{Field, bit};
    pub const START: u32 = bit(0);
    pub const PHASE: Field = Field::bits(7, 4);
    /// Mode 0..7 (state sizes 80, 160, 180, 56, 784, 392, 136, 320 bytes).
    pub const MODE: Field = Field::bits(10, 8);
    pub const DONE: u32 = bit(31);
}

/// Function-3 length: write the input sample count; after the job `[26:16]`
/// is the output count. \[code\]
pub const F3_LEN: Reg = Reg::at(BASE + 0x10);
pub const F3_LEN_OUT: Field = Field::bits(26, 16);

/// DMA mode words: memory → accelerator, accelerator → memory. \[code\]
pub const VENDOR_DMA_MODE_WRITE: u32 = 0x1D00;
pub const VENDOR_DMA_MODE_READ: u32 = 0x1D;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::assert_disjoint;

    #[test]
    fn layout() {
        assert_eq!(F3_LEN.addr(), 0xC020_0010);
        assert_eq!(ctl::FUNC.get(ctl::VENDOR_F3), ctl::FUNC_3);
        assert_eq!(ctl::UNK_3_2.get(ctl::VENDOR_IIR), 2);
        assert_disjoint(&[ctl::FUNC.mask(), ctl::UNK_3_2.mask(), ctl::UNK_B30, ctl::DONE]);
        assert_disjoint(&[fft_ctl::START, fft_ctl::UNK_A, fft_ctl::UNK_B, fft_ctl::SIZE.mask()]);
        assert_disjoint(&[
            iir_ctl::START,
            iir_ctl::FLAG,
            iir_ctl::PHASE.mask(),
            iir_ctl::SECTIONS.mask(),
            iir_ctl::SAMPLES.mask(),
            iir_ctl::DONE,
        ]);
        assert_disjoint(&[f3_ctl::START, f3_ctl::PHASE.mask(), f3_ctl::MODE.mask(), f3_ctl::DONE]);
    }
}
