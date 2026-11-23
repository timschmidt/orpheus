//! Key hardware: two ADC ladders and the power-key level (01-hardware §5,
//! port `button-yotonym03.c` [HW]). Decoding is in `yoton-input`.

use atj2157_hal::pac::{Reg, cmu, gpio, pmu};
pub use yoton_input::{Debouncer, Edge, Key, Keys, Sample, decode, earphones_present};

use crate::pins;

/// Second function word for the ladder-2 pad (`0xC01C0304`). [HW]
const LADDER2_PAD: Reg = Reg::at(0xC01C_0304);

/// Route ladder 2 to its ADC and enable the LRADC channels. [HW]
pub fn init() {
    // VERBATIM(vendor KEY.DRV via the port): pad function and ADC enables.
    gpio::mfp(usize::from(pins::KEY_LADDER2)).write(0x303B);
    LADDER2_PAD.write(0x3001);
    cmu::CLKEN0.set_bits(cmu::clken0::LRADC);
    pmu::PMUADC_CTL.set_bits(0x210);
}

/// One raw sample (ladder values are 10 bit).
#[must_use]
pub fn sample() -> Sample {
    Sample {
        ladder1: (pmu::lradc_data(1).read() & 0x3FF) as u16,
        ladder2: (pmu::lradc_data(6).read() & 0x3FF) as u16,
        // Power key level: ONOFF_CTL bit 1, 1 = held. Read only [HW].
        power: pmu::ONOFF_CTL.read() & pmu::onoff::SHORT_PD != 0,
    }
}
