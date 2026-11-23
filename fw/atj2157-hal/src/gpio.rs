//! GPIO (docs/plan/03-hal-drivers.md §3, 01-hardware §5). Minimal typed
//! output and input pins; alternate functions are set by the drivers that
//! own them.

use atj2157_pac::gpio;

#[derive(Clone, Copy, Debug)]
pub struct Output {
    pin: u8,
}

impl Output {
    /// Configure `pin` as a plain GPIO output with initial level `high`.
    #[must_use]
    pub fn new(pin: u8, high: bool) -> Self {
        let p = Self { pin };
        p.set(high);
        // Function and enables only: keep drive strength, pulls and Schmitt.
        gpio::mfp(usize::from(pin)).modify(|v| {
            (v & !(gpio::mfp_bits::FUNC.mask() | gpio::mfp_bits::IN_EN))
                | gpio::mfp_bits::FUNC_GPIO
                | gpio::mfp_bits::OUT_EN
        });
        p
    }

    fn bank_bit(self) -> (usize, u32) {
        (usize::from(self.pin / 32), 1 << (self.pin % 32))
    }

    pub fn set(self, high: bool) {
        let (b, m) = self.bank_bit();
        // Set/clear registers, as the port drives outputs [HW].
        if high {
            gpio::bsr(b).write(m);
        } else {
            gpio::brr(b).write(m);
        }
    }

    /// Toggle, using the pin's input data as the current level (U-GPIO-1:
    /// whether `+0x200` is the output latch is unconfirmed).
    pub fn toggle(self) {
        let (b, m) = self.bank_bit();
        self.set(gpio::idat(b).read() & m == 0);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Input {
    pin: u8,
}

impl Input {
    #[must_use]
    pub fn new(pin: u8) -> Self {
        gpio::mfp(usize::from(pin)).modify(|v| {
            (v & !(gpio::mfp_bits::FUNC.mask() | gpio::mfp_bits::OUT_EN))
                | gpio::mfp_bits::FUNC_GPIO
                | gpio::mfp_bits::IN_EN
        });
        Self { pin }
    }

    #[must_use]
    pub fn is_high(self) -> bool {
        gpio::idat(usize::from(self.pin / 32)).read() & (1 << (self.pin % 32)) != 0
    }
}
