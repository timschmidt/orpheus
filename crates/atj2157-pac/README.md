# atj2157-pac

Hand-written register definitions for the Actions ATJ2157 (Yoton YM03).
`no_std`, no dependencies, no `unsafe` outside `Reg`/`RoReg` in `lib.rs`.
Builds for the host (constants are unit-tested) and for
`thumbv7em-none-eabihf`.

API: `Reg::at(addr)` with volatile `read`/`write`/`modify`/`set_bits`/
`clear_bits`/`get`/`set`; `RoReg` (read only, for the eFuse block);
`Field::bits(hi, lo)` for multi-bit fields; `bit(n)` masks. Each module has a
`BASE`, register constants or `const fn` accessors for multi-instance blocks,
and one field submodule per register. Vendor and port values are named
`VENDOR_*` / `PORT_*` with their provenance; there is no driver code.

Every register and field carries a trust tag: **[DS]** datasheet V1.2 with
section, **[HW]** used by the working Rockbox port or verified on the player,
**[code]** vendor/ROM code, RE reports or the ATJ2127 SDK header, **[inferred]**.

## Modules

| Module | Block | Base | Main source |
|---|---|---|---|
| `rmu` | Resets | `0xC0000000` | DS §4.4 |
| `cmu` | Clocks; oscillators and PLLs at `0xC0000100` | `0xC0001000` | DS §4.2–4.3; PLLs, `SYSCLK` [code]/[HW] |
| `pmu` | PMU, charger, power states, low-rate ADCs | `0xC0010000` | DS §4.1, §9.3 |
| `efuse` | eFuse controller (**read-only**, addresses only) | `0xC0020000` | system.md §5.5 [code] |
| `rtc` | RTC, watchdog, timers T0–T3 | `0xC0030000` | DS §4.5 |
| `memctl` | SRAM2/3 windows, bus-master enables, bank tags | `0xC0040000` | port crt0 [HW], system.md §10 |
| `spicache` | SPI cache controllers (undocumented, addresses only) | `0xC0050000`, `0xC0060000` | DS §3.3–3.4 |
| `dma` | 10 DMA channels + SDMA (LCD/YUV) channel | `0xC0070000` | DS ch. 5 |
| `spi` | SPI0 (NOR, descrambler), SPI1, SPI2 | `0xC00A0000 + 0x10000·n` | DS §7.1–7.3 |
| `uart` | UART0–3 | `0xC00D0000 + 0x10000·n` | DS §7.4 |
| `twi` | TWI0/1 (I2C) | `0xC0110000`, `0xC0120000` | DS §7.5 |
| `usb` | USB device controller (FOTG210-style + Actions DMA) | `0xC0140000` | port driver [HW], usb_controller.md |
| `sd` | SD0 / SD1 hosts | `0xC0160000`, `0xC0170000` | port driver [HW], SDK header [code] |
| `dac` | DAC, headphone PA, mixer | `0xC0180000` | DS §8.1, audio.md, port |
| `adc` | ADC and analogue inputs | `0xC0181000` | DS §8.2, audio.md |
| `i2s` | I2S TX/RX | `0xC0182000`, `0xC0183000` | DS §8.3 |
| `pwm` | PWM0–8 | `0xC0190000 + 0x100·n` | DS §9.2 |
| `lcm` | LCD controller (8080 bus) | `0xC01A0000` | DS §9.1 |
| `gpio` | GPIO / MFP, WIO1, debug select | `0xC01C0000` | DS ch. 10 |
| `dsp` | FFT/IIR accelerator | `0xC0200000` | audio.md §7.1 [code] |
| `irq` | NVIC line numbers, priority helper | — | DS §3.2.1 |

## Sources

* `docs/vendor/atj2157-datasheet-v1.2.txt` (authoritative for layouts).
* `docs/hw/{system,audio,bluetooth,fm-i2c-imu}.md` (reverse engineering;
  written before the datasheet was found — disagreements are noted in place).
* The Rockbox port in `yoton/ext/rockbox/firmware/{export/atj2157.h,
  target/arm/atj2157/**}` and `yoton/re/usb_controller.md`: used for facts
  only (GPL; no text or code copied). Tests assert that every address the
  port uses matches.
* `docs/vendor/gl5110-atj2127-sdk-registers.h` (sister chip; names only).

Corrections and disagreements found while writing the crate are listed in
`docs/plan/01-hardware.md` §9 ("HAL-0 reconciliation").

## Remaining unknowns (ids from 01-hardware §9)

* No datasheet chapter: USB (U-USB-1…7, 11–13), SD (U-SD-1, U-SD-3, U-SD-4),
  memory controller (U-MEM-1), SPI caches (U-XIP-1), DSP (U-DSP-1), eFuse
  (U-EFUSE-1), image block (U-IMG-1).
* Undocumented registers used by vendor code: `CMU_SYSCLK` bit 8 / `[25:24]`
  (U-PWR-2), `CMU+0x30/0xAC/0xB0/0xB8`, audio PLLs `0xC0000118/11C`, PLL SSC,
  `PMU+0x0C`, `PMU+0x18`, `RTC+0x18`, SPI0 `+0x14`, `+0x20…0x2C` (U-SPI-1),
  GPIO `+0x21C`/`+0x400` (U-GPIO-4), `DEBUG_SEL` (U-GPIO-3).
* SPI0 clock source coding of the ROM value `0x101` (U-PWR-3).
* DMA: live `RC` read-back and reload latch point (U-DMA-2), SDMA interrupt
  enable (U-DMA-3), event triggers (U-DMA-4), selectors 5/6/0x11/0x1D (U-DMA-5).
* Power key bit `ONOFF_CTL[1]`: live status per datasheet, latch per vendor (U-PMU-5).
* LCM: `AHB_CFG_DATA` on command writes (U-LCM-3), DMA timing (U-LCM-1/2).
* PWM duty update while running (U-PWM-1), breath/DMA modes (U-PWM-2).
* TWI clock source (U-TWI-1); GPIO `DNMOS` (U-GPIO-2).
* Audio: PA gain steps and ramp (U-AUD-1), > 48 kHz (U-AUD-2), ADC paths and
  `BIAS0/1` (U-AUD-4), loopbacks (U-AUD-6), `PA_ANTIPOP` bypass bits (U-FM-3),
  `DMAWIDTH` (U-AUD-7), FIFO1 mixing (U-AUD-8), FIFO counter (U-AUD-9).
