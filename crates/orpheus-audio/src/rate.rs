//! What the output path does with a stream's sample rate and word length
//! (04 §3.2).
//!
//! The DAC runs natively at 8, 11.025, 12, 16, 22.05, 24, 32, 44.1 and 48 kHz
//! (both PLL families). A native rate is never resampled. 88.2/96 kHz and
//! 176.4/192 kHz sources are decimated by 2 or 4 with a half-band FIR (DSP
//! stage, WP AU-6) unless the DAC turns out to accept them directly
//! (U-AUD-2: `direct_high` below). Anything else is refused at open with
//! `Unsupported::Rate` — there is no general resampler.
//!
//! Word length: 24- and 32-bit (and float) sources come out of the decoders
//! as left-justified `i32`; they are dithered to 16 bits (TPDF) unless the DAC
//! FIFO takes 24-bit words.

/// The DAC's native rates (Hz), ascending.
pub const NATIVE_RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatePlan {
    /// Play at this rate, no conversion.
    Native(u32),
    /// Decimate by `factor` (2 or 4) and play at `rate`.
    Decimate { factor: u8, rate: u32 },
    /// The DAC takes this high rate directly (only if U-AUD-2 confirms it).
    DirectHigh(u32),
    /// Not playable.
    Unsupported,
}

/// Decide how a stream at `rate` Hz is played. `direct_high` says whether
/// the DAC accepts 96 kHz (and 88.2 kHz) directly.
#[must_use]
pub fn rate_plan(rate: u32, direct_high: bool) -> RatePlan {
    if NATIVE_RATES.contains(&rate) {
        return RatePlan::Native(rate);
    }
    match rate {
        88200 | 96000 if direct_high => RatePlan::DirectHigh(rate),
        88200 | 96000 => RatePlan::Decimate { factor: 2, rate: rate / 2 },
        176_400 | 192_000 => RatePlan::Decimate { factor: 4, rate: rate / 4 },
        _ => RatePlan::Unsupported,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthPlan {
    /// 16-bit samples go to the FIFO unchanged.
    Pass16,
    /// Reduce to 16 bits with TPDF dither.
    Dither16,
    /// The FIFO takes 24-bit words: pass the top 24 bits.
    Pass24,
}

/// Decide the word length at the DAC for a source with `bits` significant
/// bits. `fifo24` says whether a 24-bit FIFO mode exists.
#[must_use]
pub fn width_plan(bits: u8, fifo24: bool) -> WidthPlan {
    match (bits <= 16, fifo24) {
        (true, _) => WidthPlan::Pass16,
        (false, true) => WidthPlan::Pass24,
        (false, false) => WidthPlan::Dither16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        assert_eq!(rate_plan(44100, false), RatePlan::Native(44100));
        assert_eq!(rate_plan(96000, false), RatePlan::Decimate { factor: 2, rate: 48000 });
        assert_eq!(rate_plan(96000, true), RatePlan::DirectHigh(96000));
        assert_eq!(rate_plan(176_400, false), RatePlan::Decimate { factor: 4, rate: 44100 });
        assert_eq!(rate_plan(64000, false), RatePlan::Unsupported);
        assert_eq!(width_plan(24, false), WidthPlan::Dither16);
        assert_eq!(width_plan(8, false), WidthPlan::Pass16);
    }
}
