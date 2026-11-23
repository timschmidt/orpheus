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

    pub fn set_duty(self, d: u32) {
        duty(self.n).modify(|v| (v & !0xFFFF) | (d & 0xFFFF));
    }

    /// Start output (control word `0x1C` as the port [HW]).
    pub fn enable(self) {
        ctl(self.n).modify(|v| (v & !3) | 0x1C);
    }
}
