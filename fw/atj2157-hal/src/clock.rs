//! Clocks. For now: recreate the state the boot ROM leaves in USB recovery
//! (`atj_rom_usb_clock_setup` in the port, 01-hardware §3) so a cold NOR
//! boot and a RAM load start alike. The governor (WP-PWR-1) comes later.

use atj2157_pac::{cmu, memctl, pmu};

/// ROM-state recreation [HW]: USB supplies on, COREPLL 48 MHz, system clock
/// from COREPLL, DMA + USB clocks, bus-master memory enables.
pub fn rom_state() {
    // VERBATIM(vendor ROM state, via the Rockbox port's atj_rom_usb_clock_setup)
    pmu::VOUT_CTL.set_bits(pmu::vout::USBVCC_EN);
    crate::delay::spin_us(20);
    pmu::VOUT_CTL.set_bits(pmu::vout::USBVDD_EN);
    if cmu::COREPLL_CTL.read() != 0x88 {
        cmu::COREPLL_CTL.write(0x88);
        crate::delay::spin_us(1000);
    }
    cmu::SYSCLK.write(0x0300_0002);
    cmu::CLKEN0.set_bits(cmu::clken0::DMA | cmu::clken0::USB);
    memctl::MASTER_EN.set_bits(0xFF);
    crate::delay::set_core_hz(48_000_000);
}
