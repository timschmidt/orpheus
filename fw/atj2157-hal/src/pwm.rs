//! PWM channels, `0xC0190000 + 0x100·n` (datasheet §9.2; port backlight).
//! Register constants local until WP-HAL-0 (TODO(pac)).

use atj2157_pac::{Reg, cmu, rmu};

const fn ctl(n: usize) -> Reg {
    Reg::at(0xC019_0000 + 0x100 * n)
}
const fn period(n: usize) -> Reg {
    Reg::at(0xC019_0014 + 0x100 * n)
}
const fn duty(n: usize) -> Reg {
    Reg::at(0xC019_0018 + 0x100 * n)
}

/// `CTL` bit 4: the channel runs (`CH_START`, datasheet §9.2.6.1).
const CTL_CH_START: u32 = 1 << 4;

#[derive(Clone, Copy, Debug)]
pub struct Pwm {
    n: usize,
}

impl Pwm {
    /// Channel `n` with `period_steps` duty steps. Clock word as the vendor
    /// backlight driver uses it (`0x20F`) [HW via the port].
    #[must_use]
    pub fn new(n: usize, period_steps: u32) -> Self {
        if cmu::CLKEN0.read() & cmu::clken0::PWM == 0 {
            cmu::CLKEN0.set_bits(cmu::clken0::PWM);
            rmu::RST0.clear_bits(rmu::rst0::PWM);
            crate::delay::spin_us(10);
            rmu::RST0.set_bits(rmu::rst0::PWM);
        }
        // VERBATIM(vendor DRV_LCD backlight via the port): source/divider.
        cmu::pwm_clk(n).modify(|v| (v & !0x1FF) | 0x200 | 0xF);
        period(n).modify(|v| (v & !0xFFFF) | (period_steps & 0xFFFF));
        Self { n }
    }

    /// Write `DUTY[15:0]` as it is. Whether a write to a running channel
    /// takes effect is unmeasured (U-PWM-1): to change a running channel's
    /// duty use [`Pwm::set_duty_restart`].
    pub fn set_duty(self, d: u32) {
        duty(self.n).modify(|v| (v & !0xFFFF) | (d & 0xFFFF));
    }

    /// Change the duty as the stock firmware does (`DRV_LCD.DRV` bank 1,
    /// LCD API 1; U-PWM-1, U-PWM-3): `CTL` bit 4 (`CH_START`) cleared,
    /// `DUTY[15:0]` written, `CH_START` set again, with no wait between.
    ///
    /// VERBATIM(vendor DRV_LCD API 1): the purpose of the stop/start is not
    /// understood — the datasheet says `CH_START` = 0 takes effect only
    /// after the current period (§9.2.6.1) — and whether a plain `DUTY`
    /// write to a running channel would do (U-PWM-1) is unmeasured.
    pub fn set_duty_restart(self, d: u32) {
        ctl(self.n).clear_bits(CTL_CH_START);
        duty(self.n).modify(|v| (v & !0xFFFF) | (d & 0xFFFF));
        ctl(self.n).set_bits(CTL_CH_START);
    }

    /// Start output (control word `0x1C` as the port [HW]).
    pub fn enable(self) {
        ctl(self.n).modify(|v| (v & !3) | 0x1C);
    }
}
