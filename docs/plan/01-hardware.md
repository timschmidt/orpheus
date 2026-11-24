# 01 — Hardware reference and unknowns register

One page to find any hardware fact, how far it can be trusted, and what is
still unknown. Register-level detail lives in the documents this page points
to; do not copy tables from them into code comments, cite them.

## 1. Sources and how far to trust them

| Tag | Meaning | Where |
|---|---|---|
| **[DS]** | Stated in the vendor datasheet (ATJ2157 Datasheet V1.2, 2022-04-30, 336 pages). Local text copy: `docs/vendor/atj2157-datasheet-v1.2.txt` (not for redistribution; public source: datasheet4u.com, part ATJ2157). Chapters: PMU, oscillators, CMU, RMU, RTC/watchdog/timers, DMA, SPI, UART, TWI, DAC, ADC, I2S, LCD, PWM, LRADC, GPIO/MFP, pin list, interrupt table, memory map. **No register chapters for SD, USB, NAND, the cache controllers, the memory controller or the hardware engines** | `docs/vendor/` |
| **[HW]** | Exercised on this player by the Rockbox port or a test payload | `/home/tim/Documents/GitHub/yoton/` — `FINDINGS.md`; `ext/rockbox/firmware/export/atj2157.h`; `ext/rockbox/firmware/target/arm/atj2157/**`; `payloads/*.c` |
| **[code]** | Read from vendor or ROM code, never run by us | `docs/hw/system.md`, `audio.md`, `bluetooth.md`, `fm-i2c-imu.md`; `yoton/re/usb_controller.md` |
| **[inferred]** | Deduced; treat as a hypothesis with a test | same documents, marked in place |
| **[unknown]** | Listed in §9 with the test that settles it | |

For every block the datasheet covers, **the datasheet is the primary source**
for register names, fields and reset values; the reverse-engineering reports
then tell which fields the stock firmware actually uses and in what order.
Where the two disagree, test, and record the outcome here. The reports in
`docs/hw/` were written before the datasheet was found: their "[unknown]" and
"[inferred]" marks on documented blocks are superseded by it (first PAC task:
reconcile them, block by block).

Rule: a driver may rely on **[HW]** facts; it may start from **[DS]** and
**[code]** facts but its bring-up image must demonstrate them; it must not depend on an
**[inferred]** fact without running the test first and recording the result
here and in the source document.

Raw material for further reverse engineering (vendor files in
`yoton/re/fw/`, ROM and NOR dumps in `yoton/dumps/`, module/bank layout in
each `docs/hw/*.md` appendix A). Ghidra headless does not run on this host
(needs a full JDK 25); the reports were made with `objdump`/capstone scripts.

## 2. The device

| Item | Value | Trust |
|---|---|---|
| Product | Yoton YM03 clip-on player, 370 mAh Li-ion, microSD slot (no internal storage), 3.5 mm jack, USB | vendor |
| SoC | Actions ATJ2157: Cortex-M4 r0p1 with FPU (single precision), MPU, no cache; boot-ROM id `0x3051` | [HW] |
| Core clock | 48 MHz as left by the ROM; datasheet "up to 288 MHz"; vendor table goes to 300 MHz with core-voltage codes | 48: [HW]; rest [DS]/[code] |
| Interrupts | 64 lines, **4 priority levels** (2 bits) | [DS] |
| RAM | 320 KiB in four blocks (02 §1.1). The datasheet lists 224 KiB RAM + a 32 KiB "HW RAM" + a **32 KiB I-cache and 32 KiB D-cache (4 ways × 8 KiB)**; the measured SRAM2/SRAM3 blocks, which alias in 8 KiB slots, are almost certainly those cache RAMs used as plain memory | [HW] / [DS] |
| ROM | 64 KiB mask ROM at 0; USB recovery ("ADFU") always available | [HW] |
| Flash | 8 MiB SPI NOR, JEDEC `5E 40 17` (Zbit), on SPI0; not memory-mapped; vendor data scrambled, hardware descrambler in the SPI block | [HW] |
| Display | 128 × 128 RGB565, ST7735S-class, 8080 bus on the LCM controller, panel offset x+2 / y+3 with MADCTL `0xC8`, backlight on PWM4 | [HW] |
| Keys | Power (PMU level) + 9 keys on two ADC ladders | [HW] |
| Card | SD0 host, 4-bit, 24 MHz verified; card detect on GPIO18 (config) | [HW] / [code] |
| USB | High-speed device, FOTG210-style core + Actions DMA, IRQ 12 | [HW] |
| Audio out | Internal DAC + headphone amplifier at `0xC0180000`; 8–48 kHz native; 8-bit digital volume per channel, 3-bit amplifier gain | play: [HW]; volume: [code] |
| Audio in | ADC and microphone/FM/line front end at `0xC0181000` | [code] |
| FM | RDA5807-family tuner at I2C `0x10`, 24 MHz reference, analogue output into the headphone mixer | [code] |
| Motion | MiraMEMS DA2xx-family 3-axis accelerometer at I2C `0x27`, polled; **no gyroscope** | [code] |
| I2C | SCL = GPIO24, SDA = GPIO25. The stock firmware bit-bangs them, but the pin list gives these pads the functions **TWI1_CLK / TWI1_DAT**: the hardware controller TWI1 (`0xC0120000`, IRQ 20) can drive the bus | [code] / [DS] |
| Bluetooth | Autonomous transmitter module ("ATS2851") on UART1, analogue audio feed | [code] + RAM dump |
| RTC | SoC block at `0xC0030000`, binary calendar, alarm IRQ 6 | [code] |
| Hardware engines | Datasheet feature list: "FFT/FIR/IIR hardware engine", AES, TRNG, chip unique id, Motion-JPEG decoder, scaler, thermal sensor — **none has a register chapter**. The vendor audio driver holds unused code for the FFT/IIR engine at `0xC0200000` | [DS] feature list; [code] |
| Debug | UART0 TX disabled in the shipped configuration; SWD mappable to GPIO62/63, 55/56 or 16/17 by `0xC01C0000`; pads not located | [code] |

## 3. Memory and SRAM2/3 bring-up

Map: 02-architecture §1.1. After a cold boot through our boot stages SRAM2 and
SRAM3 are **off**. Before any use (first instructions of crt0, as
`crt0-atj2157.S` does) **[HW]**:

```
CMU_CLKEN0 (0xC0001008) |= 0x600            clocks for SRAM2, SRAM3
RMU_RST0   (0xC0000000) : clear 0x600, set 0x600
0xC0040010 = 0x00200000   0xC0040014 = 0x00207FFF      SRAM2 window
0xC0040018 = 0x00300000   0xC004001C = 0x00307FFF      SRAM3 window
0xC0040300 = 0x00200000   0xC0040304 = 0
0xC0040308 = 0x00300000   0xC004030C = 0
```

Also needed after a cold boot before USB or DMA into RAM works **[HW]**
(`atj_rom_usb_clock_setup` in the port): `PMU+0x00` bits 29 then 28,
`0xC0000114 = 0x88` (COREPLL 48 MHz), `CMU_SYSCLK = 0x03000002`,
`CMU_CLKEN0 |= 0x8001`, `0xC0040004 |= 0xFF` (bus-master memory enables).

The memory controller also has **bank-tag registers** (`0xC0040080 + 4n`) with
which the vendor OS maps virtual windows onto SRAM0 and pages on UsageFault
(system.md §10). Orpheus does not use them in the baseline; they are a
possible way to add SRAM0-backed code cache later (U-MEM-1).

## 4. Peripheral blocks

| Block | Base | Clock gate / reset | Reference | Status |
|---|---|---|---|---|
| RMU (resets) | `0xC0000000` | — | system.md §2 | partly [HW] |
| Oscillator / PLLs | `0xC0000100–0x124` | — | system.md §1 | 48 MHz [HW], rest [code] |
| CMU | `0xC0001000` | — | system.md §1–2 | partly [HW] |
| PMU | `0xC0010000` | — | system.md §3 | partly [HW] |
| eFuse controller | `0xC0020000` | CLKEN1 b31 | system.md §5.5 | **read-only for us** |
| RTC, watchdog, timers T0–T3 | `0xC0030000` | CLKEN0/RST0 b24 (timers) | system.md §5 | T0, watchdog [HW] |
| Memory controller | `0xC0040000` | — | system.md §10 | windows [HW] |
| SPI cache controllers (hardware XIP) | `0xC0050000`, `0xC0060000` | datasheet CMU/RMU bits | **no register documentation**; §9 U-XIP-1 | unexplored |
| DMA (10 channels + LCD special channel) | `0xC0070000`, `0xC0071000` | CLKEN0/RST0 b0 | datasheet ch. 5; audio.md §6 | ch 0, 4, 5 [HW] |
| TWI0 / TWI1 | `0xC0110000` / `0xC0120000` | datasheet | datasheet §7.5 | unused by stock |
| Image/video block | `0xC0090000` | CLKEN0/RST0 b12 | system.md §2 | not analysed (U-IMG-1) |
| SPI0 (NOR) | `0xC00A0000` | b6 | system.md §9; `spinor.h`; pager | [HW] |
| UART0 / UART1 | `0xC00D0000` / `0xC00E0000` | b20 / b21 | system.md §7; bluetooth.md §2 | [code] |
| USB | `0xC0140000` | b15 (+RST0 b16), PHY clock `0xC0001100` | usb_controller.md; port driver | [HW] |
| SD0 | `0xC0160000` | b2 | port `sdmmc-yotonym03.c` | [HW] |
| DAC + amplifier + mixer | `0xC0180000` | CLKEN1 b0/2/11, RST1 b0/2 | audio.md §1–2, §4 | playback [HW] |
| ADC + inputs | `0xC0181000` | CLKEN1 (audio.md) | audio.md §3 | [code] |
| I2S TX / RX | `0xC0182000` / `0xC0183000` | — | audio.md §5 | pins taken on this board; unused |
| PWM | `0xC0190000 + 0x100·n` | b25 | port backlight | PWM4 [HW] |
| LCM (8080) | `0xC01A0000` | b13 | port `lcd-yotonym03.c`; system.md §1.5 | PIO [HW], DMA [code] |
| GPIO / MFP | `0xC01C0000` | — | system.md §8; fm-i2c-imu.md §1.1 | partly [HW] |
| DSP accelerator | `0xC0200000` | CLKEN1/RST1 b13, `CMU+0xAC` | audio.md §7.1 | untested |

`atj2157-pac` gets one module per block; every field carries its trust tag in
a doc comment and a pointer to the source section.

## 5. Pins

| GPIO | Use | MFP word / function | Trust |
|---|---|---|---|
| 0–7 | LCD D0–D7 | function 2 | [HW] |
| 8, 10, 11 | LCD RS, RD, WR | function 2 | [HW] |
| 43 | LCD CS | function 2, drive 2 | [HW] |
| 21 | LCD reset | GPIO out | [HW] |
| 22 | Backlight | PWM4 (function `0x10`) or GPIO out low = off | [HW] |
| 12–15 | SPI0 to the NOR | `0x3027` | [HW] |
| 17 | Key ladder 2 → ADC `PMU+0x64` | `0x303B`, with `0xC01C0304 = 0x3001` | [HW] |
| (dedicated) | Key ladder 1 + earphone sense → LRADC1 `PMU+0x50` | — | [HW] / [code] |
| 18 | SD card detect (config); stock UART0 TX option | — | [code] |
| 20 | Bluetooth module enable (high = on) | GPIO out | [code] |
| 24, 25 | I2C SCL, SDA (bit-bang; MFP bit 6 out-enable, bit 7 in-enable) | GPIO | [code] |
| 27, 28, 29, 33 | SD D3, D2, D1, D0 | `0x804` + drive 3 | [HW] |
| 30, 32 | SD CLK (drive 5), CMD | `0x804` | [HW] |
| 31 | Bluetooth host-busy output | GPIO out | [code] |
| 38, 39 | UART1 to the Bluetooth module (which is TX: U-BT-1) | `0x302A`; +`0x100` on RX | [code] |
| 48 | UART0 RX (stock config) | — | [code] |
| 62/63, 55/56, 16/17 | SWD options via `0xC01C0000` bits [1:0], enable bit 4 | — | [code] |

GPIO registers: MFP word per pin at `0xC01C0004 + 4·pin` (function [4:0],
out-enable bit 6, in-enable bit 7, drive [14:12]); data out `+0x200`
(set `+0x210`, clear `+0x220`), data in `+0x230`, per 32-pin bank. Note the
port's header names `+0x200` "OUTEN"; vendor use says it is the output latch
(U-GPIO-1).

Keys **[HW]** (idle values LRADC1 ≈ 742, ADC2 ≈ 838): ladder 1: Back ≈ 0,
Next ≈ 125, Vol+ ≈ 245, Vol− ≈ 370; ladder 2: Menu ≈ 0, Prev ≈ 140,
Down ≈ 280, OK ≈ 421, Up ≈ 561. Power: `PMU+0x2C` bit 1 (write 1 to clear the
latch). Earphone present: LRADC1 in 385–591 with no key **[code]** (U-AUD-5).

## 6. Interrupts and DMA

| IRQ | Source | Trust |
|---|---|---|
| 1 | Watchdog (2 Hz / IRQ mode) | [DS] |
| 2, 3, 4, 5 | Timers T0–T3 | [DS]; T0 [HW] |
| 6 | RTC (alarm) | [DS] |
| 7, 8 / 10, 11 | I2S RX0, RX1 / TX0, TX1 | [DS] |
| 12 | USB | [DS], [HW] |
| 13, 14, 15 | SPI0, SPI1, SPI2 | [DS] |
| 16, 17, 18 | NAND, SD card, SDIO | [DS] |
| 19, 20 | TWI0, TWI1 | [DS] |
| 21–24 | UART0–UART3 | [DS] |
| 26–35 | DMA channels 0–9 (5 → 31 [HW]) | [DS] |
| 36 | External interrupt (GPIO) | [DS] |
| 39, 40, 41 | DAC, ADC01, ADC23 | [DS] |
| 42 | LCD | [DS] |
| 48, 50 | "MPU", "SDMA" (as printed; meaning unclear) | [DS] |
| 0, 9, 25, 37, 38, 43–47, 49, 51–63 | none / not connected: candidates for the software-pended RT executor line | [DS] |

Design consequence: the table is from the datasheet; the stock firmware uses
only a few of these lines (it polls keys, SD, UART and the charger), so each
line Orpheus relies on beyond T0, USB and DMA 5 is confirmed in its driver's
bring-up image before the driver depends on it.

DMA channel plan (any channel can serve any device; the mode word selects it):

| Ch | Use | Mode word | Trust |
|---|---|---|---|
| 0 | SPI0 RX → memory: **the pager and NOR driver only** | `0x00200088` | [HW] |
| 1 | Frame buffer → LCM | `0x9600` | [code] |
| 2 | Spare (SPI0 TX for programming if useful) | — | |
| 3 | DSP accelerator (vendor uses 9; device codes `0x1D00` / `0x1D`) | [code] | |
| 4 | SD0 | `0x85` read, `0x8500` write | [HW] |
| 5 | DAC FIFO `0xC018000C` | `0x00148B00` (16-bit stereo) | [HW] |
| 6 | ADC FIFO `0xC0181018` | `0x0014008B` | [code] |
| 7 | Spare | — | |
| 8 | UART1 RX, circular | `0x00200082` | [code] |
| 9 | UART1 TX | `0x00228200` | [code] |

Mode-word fields **[DS]** (datasheet §5.3 `DMAxCTL`): [4:0] source select and
[12:8] destination select (0 memory, 1–4 UART0–3, 5 SDIO, 6 SD/MMC, 8–10
SPI0–2, 0x0B DAC FIFO0 / ADC01, 0x0C DAC FIFO1 / ADC23, 0x0D–0x0E I2S, 0x13–
0x14 TWI0–1, 0x15 PWM, 0x16 LCD, 0x18 NAND), bit 7 / bit 15 source /
destination address constant, bit 16 planar audio layout, bit 17 single
instead of burst-8 transfers, **bit 18 reload** (re-arm and restart when the
count runs out), [21:20] width (0 = 32-bit, 1 = 16-bit, 2 = 8-bit). The
start register's bit 0 clears itself on completion or error; writing 0 aborts.
Channel registers: mode `+0x00`, control `+0x04` (bit 0 start/busy), source
`+0x08`/`+0x0C`, destination `+0x10`/`+0x14`, length `+0x18`, remaining
`+0x1C`. Pending/enable: bit n and bit n + 16.

DMA addresses: SRAM0 and SRAM1 as CPU addresses; SRAM2 at `0x10000000`,
SRAM3 at `0x18000000`.

## 7. Clocks and power in brief

HOSC 24 MHz; a 32.768 kHz oscillator and an internal 32 kHz RC; COREPLL and
DEVPLL = 6 MHz × N. `CMU_SYSCLK` bits [1:0] select 32 kHz / HOSC / COREPLL /
DEVPLL, [5:4] divide by 1–8. Peripheral clocks (SPI0, SD, LCD from DEVPLL or
HOSC; UART and timers from HOSC; audio from the audio PLLs) do **not** follow
the core clock. Detail: system.md §1; datasheet §4.2–4.3.

Core supply: an on-chip buck converter (switchable to LDO mode), 0.7–1.35 V.
`VOUT_CTL` (`PMU+0x00`) bits [4:0] set VDD in run state in 25 mV steps from
0.7 V **[DS]**:

| Code | VDD | Vendor uses it for |
|---|---|---|
| `0x0C` | 1.000 V | everything up to 144 MHz |
| `0x10` | 1.100 V | 156–192 MHz |
| `0x14` | 1.200 V (**reset default**) | 204–228 MHz |
| `0x16` | 1.250 V | 240–276 MHz |
| `0x18` | 1.300 V | 288–300 MHz |

So the Rockbox port's 48 MHz at the reset default runs the core at 1.2 V where
the stock firmware uses 1.0 V: lowering the code to `0x0C` is the first and
largest power saving, and it is a vendor-used value. Bits [9:5] set VDD for
the S3 standby state (default 0.9 V); [12:10] VCC (3.1 V default), [19:17]
AVCC, bits 28/29 USB supplies.

Power states **[DS]**: `POWER_CTL` (`PMU+0x20`) has `EN_S1` (bit 0, run) and
`EN_S3` (bit 1, standby). The stock firmware only ever powers off (clears
both). **S3 is a documented standby state the stock firmware does not use**;
whether RAM survives it and how the SoC resumes is U-PMU-2. Wake sources
(`WKEN_CTL`, `PMU+0x24`): bit 0 power key long press, bit 1 short press, bit 2
DC5V inserted, bit 3 DC5V removed, **bit 4 RTC alarm**, bit 9 CPU interrupt,
bit 11 WIO1 pin, bit 23 "remote key" on LRADC1; pending flags in the same
positions in `WAKE_PD` (`PMU+0x28`), whose bit 30 is the live DC5V state. The
vendor writes 5 (long press + DC5V in) before power-off.

Power key **[DS]** (`ONOFF_CTL`, `PMU+0x2C`): bit 1 pressed; [8:6] long-press
time (0.125–4 s); [5:4] long-press function (reset / restart) after [3:2]
8/12/16/24 s — a hardware escape from a hung firmware.

Charger **[DS]** (`CHG_CTL`, `PMU+0x10`): current codes 0–7 = 25, 50, 75, 100,
200, 300, 400, 450 mA; the stock firmware uses code 4 = **200 mA** (0.54 C for
the 370 mAh cell) and code 1 = 50 mA below 3.0 V-equivalent; end voltage and
termination fields are documented; `CHG_DET` (`+0x14`) gives phase, current
band and end-of-charge.

Watchdog **[DS]** (`RTC+0x1C`): period codes 0–6 = 0.176, 0.352, 0.703, 1.4,
2.8, 5.6, 11.2 s, code 7 = 10 ms; bit 4 enable; bit 5 selects interrupt
instead of reset; bit 6 interrupt pending.

## 8. Corrections to earlier notes

* `0xC0020000` is the eFuse controller, not battery-backed registers. There
  are no backup registers. Do not write to it. (`payloads/bkflag.c` and the
  "persistent ADFU flag" in FINDINGS §10 act on a latch of eFuse word 3 whose
  persistence is unverified: U-EFUSE-1.)
* "GPIO24/25" in FINDINGS §9 are the I2C pins, not keys.
* The port's comment "no FM tuner on this model" is wrong: the stock firmware
  has a complete RDA5807 driver and the radio in its main menu.
* `GPIO_OUTEN` in the port's header is probably the output data latch.
* The strings `WAKE_PD`, `DC5V_ON`, `welcome s4` belong to the vendor's second
  boot stage (NOR `0x200–0x71FF`), which our stage 1 bypasses: its work
  (battery check, DEVPLL, SRAM2/3, pin table) is ours to do.

## 9. Unknowns register

Each entry: what is unknown, why it matters, the test, and the work package
that runs it. Results are written back here (replace the test with the answer
and date) and into the source document. Tests are bring-up images or `xtask
dev poke/peek` sessions; anything needing instruments or the case open is a
**user check** (docs/USER-CHECKS.md).

### CPU, memory, interrupts

| Id | Question | Why | Test | WP |
|---|---|---|---|---|
| U-CPU-1 | ~~NVIC priority bits~~ **Answered [DS]: 4 levels.** Confirm by read-back | Exactly the four levels of 02 §4.1 | Write `0xFF` to an IPR byte, read back (expect `0xC0`) | HAL-1 |
| U-CPU-2 | Does WFI stop the core clock; which sources wake it | Idle power | `payloads/wfitest.c` result; repeat with timer-only wake; idle counter vs wall time | HAL-1 |
| U-IRQ-1 | Interrupt table of §6 is from the datasheet; each line is unproven until used | Interrupt-driven SD, TWI, UART, key wake | Per driver: enable the block's interrupt with the NVIC line masked, cause the event, read `NVIC_ISPR` | each driver |
| U-IRQ-2 | Which unconnected NVIC line (0, 9, 25, 37, 38, 43–47 …) can be pended in software for the RT executor | 02 §4.1 | Set its `ISPR` bit, confirm the handler runs and nothing else raises it | HAL-1 |
| U-XIP-1 | **Hardware execute-in-place**: the datasheet says code can run directly from SPI NOR through a 32 KiB 4-way I-cache with an "SPI Cache" controller at `0xC0050000`, but documents no registers; the stock firmware does not use it (it pages in software) | If it can be made to work it replaces the software pager with hardware and removes page-fault latency | **Time-boxed investigation (two sessions), not on the critical path**: dump `0xC0050000–0xC00500FF` and `0xC0060000…` in ROM state and under the stock firmware; look in the datasheet's CMU/RMU chapters for the cache clock/reset bits; search the ROM and the leaked ATJ2127 SDK header (`gl5110.h`, docs/research/rust-ecosystem.md §10) for the block; try enabling it with SRAM2 windows disabled and a known pattern at NOR offset 0x762000. Stop at the first dead end and record what was learned | XIP-1 |
| U-MEM-1 | Bank-tag windows: size, count, usable beside the MPU pager | Extra code cache | system.md T8 | later |
| U-EFUSE-1 | Is the word-3 latch volatile | Safety of existing tools | system.md T7 (read only) | T-2 |
| U-GPIO-1 | Is `0xC01C0200` the output data latch (vendor use, datasheet GPIO chapter) rather than an output enable (name in the port's header) | Correct GPIO driver | Read datasheet §10; toggle a spare pin and read it back | HAL-1 |

### Clocks and power

| Id | Question | Why | Test | WP |
|---|---|---|---|---|
| U-PWR-1 | ~~Core voltage per code~~ **Answered [DS]** (§7 table; reset default `0x14` = 1.2 V). Remaining: read `PMU+0x00` after a cold boot through our stages; confirm the player is stable at `0x0C` (1.0 V) for every level ≤ 144 MHz | Power saving; safe levels | Level self-test (03 §2.2) at each level and code; optional **user check** of the VDD rail with a meter | PWR-1 |
| U-PWR-2 | `CMU_SYSCLK` bit 8 and [25:24] | Bus clock at high levels | system.md T2 | PWR-1 |
| U-PWR-3 | SPI0 clock source/divisor encoding | Pager read speed at each configuration | system.md T3 | PG-1 |
| U-PWR-4 | Can DEVPLL / COREPLL be off, HOSC divided, 32 kHz selected, with what wake-up | Deep idle | Stepwise in a bring-up image with timer wake; current needs a meter (**user check**), software proxy = battery ADC slope | PWR-3b |
| U-PMU-1 | ~~Charge current per code~~ **Answered [DS]** (code 4 = 200 mA, code 1 = 50 mA). Remaining: end-voltage field (`CHG_VLOT`, `STOPV`) values the stock firmware leaves; behaviour of `CHG_DET` during a full charge | Safety, UI | Dump `CHG_CTL`/`CHG_DET` under the stock firmware while charging (boot stock, then ADFU read is not possible — read them from Orpheus right after a stock session with charger running, or decode the vendor writes); log a full charge cycle | PWR-2 |
| U-PMU-2 | **S3 standby** (`POWER_CTL` bit 1, `VOUT_CTL[9:5]`): does RAM survive; does the SoC resume in place or restart through the ROM; which wake sources work; current drawn | Standby with alarms, background step counting, instant resume | Bring-up image: write a RAM pattern and a mailbox marker, arm the RTC alarm 10 s ahead with `WKEN_CTL` bit 4, enter S3 (clear `EN_S1` with `EN_S3` set — derive the exact sequence from the datasheet text first), observe over USB what comes back and whether the pattern survived. Needs a person only if nothing wakes it (reset pinhole) | PWR-3c |
| U-PMU-3 | Wake from **off**: bits are documented (§7); confirm DC5V-insert, alarm and key wake on this board | Alarms, charging screen, unattended tests | Alarm case is fully automatic (arm alarm, power off, watch USB re-enumerate); DC5V case needs a re-plug (**user check**) | PWR-3a |
| U-PMU-4 | ~~Long-press timing~~ **Answered [DS]** (§7). Choose values: long press 1.5 s, hardware reset after 8 s | Recovery UX | **User check**: hold Power 8 s on a deliberately hung test image | PWR-3a |
| U-RTC-1 | RTC keeps time when off; oscillator-stopped flag; LOSC accuracy | Clock, TOTP | system.md T6, T13 | RTC-1 |
| U-WDT-1 | ~~Watchdog periods~~ **Answered [DS]** (§7). Remaining: interrupt mode (IRQ 1) behaviour | Early-warning dump before a watchdog reset | Set bit 5, let it expire, observe IRQ 1 | HAL-1 |

### Storage, display, DMA

| Id | Question | Why | Test | WP |
|---|---|---|---|---|
| U-NOR-1 | What the 4 KiB erase (`0x20`) and 64 KiB erase do on this chip; status-register protection bits | Settings store geometry | 4 KiB erase is used by `norprog.c` **[HW]**; read SFDP and status registers | PG-1 |
| U-NOR-2 | Program/erase suspend and resume (`0x75`/`0x7A`) and their latencies | Paging during NOR writes (02 §3.2 rule 5) | In the scratch sector: start an erase, suspend, read another sector, resume, verify | PG-2 |
| U-NOR-3 | Descrambler: is the key stream position-dependent only (random-offset reads of vendor data equal the dump) | In-place use of the vendor font | Read 100 random 33-byte spans with CTL bit 12, compare with `nor_descrambled_8m.bin` | UI-1 |
| U-SD-1 | High-speed (50 MHz class) operation with DEVPLL clock; CMD6 switch; controller timing bits | Card-reader speed | Bring-up test on the scratch file region with CRC | SD-2 |
| U-SD-2 | Card-detect polarity on GPIO18; hot-plug behaviour | Hot-plug | Read the pin with and without a card (**user check** for the removal) | SD-1 |
| U-LCM-1 | LCM timing fields (`0xC01A0008/0C`, CTL bits) versus core clock; can a blit run without dropping the core to HOSC as the vendor does | Display at high clock levels | Blit at 48, 96, 144 MHz with timing swept; verify by panel read-back of a test pattern (the panel supports RAM read; `payloads/lcdread.c`) | LCD-1 |
| U-LCM-2 | DMA blit: channel 1 mode `0x9600` vs the special channel; RGB565 byte order in DMA mode | Zero-CPU flush | Bring-up with read-back | LCD-1 |
| U-DMA-1 | DAC DMA from SRAM3 (`0x18000000`) | PCM ring placement | Tone test with the ring in SRAM3; cadence counters | AU-1 |
| U-DMA-2 | Reload mode is documented (bit 18); remaining: what the second pending bit (n + 16) signals, whether the remaining-count register is readable while running, whether source/length can be changed for the *next* lap while a lap runs | Circular audio/UART buffers without refill deadlines | Loop a buffer on the DAC channel, sample the count, observe both pending bits; read datasheet §5.3 `DMAIP` first | AU-1 |

### USB

U-USB-1 … U-USB-10 are questions 1–10 of `usb_controller.md` §10 (short packet
ending a multi-packet OUT DMA; control transfers > 64 bytes and zero-length
packets; DMA interrupts; unexplained registers; reset bits; DMA reach beyond
SRAM0; meaning of "IN done"; clearing of `CX_SETUP`; exact VBUS bit; endpoint
halt mapping). The port's working driver answers 8 and 10 in practice; record
that. Added:

| Id | Question | Test | WP |
|---|---|---|---|
| U-USB-11 | Interrupt-type endpoint/FIFO configuration (FIFO type field) and timing | HID keyboard enumerates and types on the host | USB-5 |
| U-USB-12 | Isochronous FIFOs with the Actions DMA; feedback endpoint | UAC1 speaker plays a tone (stretch) | USB-7 |

### Audio, radio, sensor, Bluetooth

| Id | Question | Test | WP |
|---|---|---|---|
| U-AUD-1 | Digital volume scale (is `0xBF` 0 dB, 0.375 dB/step), ramp behaviour, PA gain steps | Digital loopback or ADC capture of a tone at several settings; else **user check** by ear with a reference table | AU-1 |
| U-AUD-2 | IC-type gate for > 48 kHz and 24/32-bit FIFO format (audio.md §1.4) | Read the IC type; try 96 kHz and the wide format with cadence counters | AU-1 |
| U-AUD-3 | Power-down anti-pop sequence | **User check** by ear | AU-1 |
| U-AUD-4 | ADC path: input select, gains, microphone bias, capture DMA. **Built 2026-11-24 (`atj2157_hal::adc`, vendor sequences VERBATIM; DMA ch 6, reload over a two-half ring in the card task's arena), not yet run** | Record silence, speech, FM (`rec start …`, `rec status`); inspect files on the host | AU-13 |
| U-AUD-5 | Earphone-sense window on this unit; behaviour with 4-pole headsets | Log LRADC1 with/without earphones (**user check** to plug) | INPUT-1 |
| U-AUD-6 | Any analogue or digital loop from DAC to ADC for self-test | audio.md §4; try the routes | AU-13 |
| U-DSP-1 | Does the accelerator at `0xC0200000` work; number formats, scaling, speed | Bring-up: FFT of known vectors, biquad impulse responses, compare with software | DSP-1 |
| U-FM-1 | Tuner present; chip id; pull-ups on GPIO24/25; **does hardware TWI1 work on these pads** (pin functions TWI1_CLK/TWI1_DAT per datasheet) | Hardware I2C instead of bit-banging | Bit-bang scan first (known-good method), then the same scan through TWI1 | FM-1 |
| U-FM-2 | Tuner reference clock present without further set-up | Tune a strong local station, read RSSI/stereo flags: `xtask dev cmd "fm tune KHZ"` then `"fm status"` (RSSI, `st=`, 0Ah/0Bh) | FM-1 |
| U-FM-3 | Meaning of mixer bits `+0x2C[23:16]`; FM → ADC route. FM-1 switches the bypass with the vendor's three steps but without its amplifier power cycle and DIGCTL[3:2] = 2 (`atj2157_hal::dac::fm_*`): if FM is silent, those two first | Toggle with a station tuned (`fm route off` / `fm route on`); capture through the ADC | FM-1 |
| U-FM-4 | Does the 0x5804 part decode RDS (RDA5807M class: it also answers at 0x11 and 0x60)? | `fm status` on a station that sends RDS: `groups=` grows, `ps="…"` | FM-1 |
| U-ACC-1 | Accelerometer id (register `0x01`), exact part, INT pin wiring | Read; enable an interrupt and scan GPIO inputs while the user taps (**user check**) | SENS-1 |
| U-FM-2 | Tuner reference clock present without further set-up | Tune a strong local station, read RSSI/stereo flags | FM-1 |
| U-FM-3 | Meaning of mixer bits `+0x2C[23:16]`; FM → ADC route | Toggle with a station tuned; capture through the ADC | FM-1 |
| U-ACC-1 | Accelerometer id (register `0x01`), exact part, INT pin wiring; **whether it has a FIFO or a step counter** (DA213/215/217/226/228 share id `0x13`) | Read; enable an interrupt and scan GPIO inputs while the user taps (**user check**). SENS-1 built `acc regs` (registers `0x00–0x3F`, reads only) and `acc read` (one sample, id, ODR, mode): their output identifies the part. The id `0x13` is answered (i2c image). Until the part is known the firmware uses only the stock firmware's configuration: no FIFO, no interrupt, polled at 25 Hz (`yoton_bsp::accel`, **VERBATIM** open sequence of fm-i2c-imu.md §3.2 steps 1–4, 7–9, 14) | SENS-1 |
| U-BT-1 | Which of GPIO38/39 is TX; BAUD register halves. **SoC side answered [DS]**: function 10 makes GPIO38 UART1_TX and GPIO39 UART1_RX (no swapped set-up exists; the other UART1 pad pairs 4/5, 22/23, 28/29 belong to the panel, backlight/keys and card); BR halves = TX/RX divisors (§9 HAL-0). Board side: `bt listen` (both pads as pulled-up inputs, module powered, falling edges counted) and `bt probe` (identity answered) | `xtask dev cmd "bt listen"`, `"bt probe"` (BT-2 device) | BT-2 |
| U-BT-4 | UART1 RX by DMA **single transfers with reload** (`0x00260082`, `CTL` `RDIC` = 1 byte) into a 256-byte ring, write position from the channel's remain counter (`+0x1C`): does the counter read correctly while the channel runs, and does reload re-arm a peripheral-paced channel (U-DMA-2 remainder)? The vendor ran burst-8 without reload and drained the tail by CPU | `bt status` after `bt on`: `ring_rem` moves, `rx bytes` grows, frames decode; else `bt rx cpu` (1 ms FIFO poll) | BT-2 — **answered 2026-11-24 [HW]: yes** — `bt probe`: 37 bytes, 5 frames, 0 bad checksums, `ring_rem` moving, 3 commands acknowledged without resend; reload re-arms the peripheral-paced channel |
| U-BT-5 | DMA channel 9 completion interrupt (line 35 [DS]) for UART TX | `bt status`: `dma_no_irq` stays 0 | BT-2 |
| U-BT-2 | ~~Analogue tap point~~ **Answered 2026-11-24: after the PA** (silent at PA volume 0, full level at 7; measured through a host A2DP sink). Still open: headphone output live while streaming | **User check** (listen) | BT-2 |
| U-BT-6 | Module wire facts seen with a BlueZ A2DP sink (2026-11-24 [HW], lead) | Address text, volume, connect | **Answered**: addresses arrive least-significant byte first (HCI order: `E0:AD:47:32:1B:1F` arrives as `1F 1B 32 47 AD E0`; commands take the bytes back unchanged, only the text is reversed); the sink's absolute volume is 0 after every connection until command 0x26 (the player sends its volume on connect, UI-6a); connect (0x03) is refused while an inquiry runs (stop it first, UI-6a); the module's SDP record advertises the HID UUID 0x1124 (06 §4.3 step 1); `bt on` reconnects to the last device within about 10 s | BT-2/UI-6a |
| U-BT-3 … | bluetooth.md §8 questions 7–10 (unknown commands, incoming connections, upgrade image, enable pin) | 06 §4.3 | BT-3 |
| U-IMG-1 | What the image/video block at `0xC0090000` does (JPEG acceleration?) | Read `MMM_VP.AL`/`MMM_ID.AL` register use; only pursue if it decodes JPEG | later |

### HAL-0 reconciliation (datasheet vs. reports and port), 2026-11-23

Written while building `atj2157-pac` (details and tags in each module's doc
comments; module list in `crates/atj2157-pac/README.md`). Nothing here was
run on the player.

**Corrections to the lead's PAC modules**

* `cmu::HOSC_CTL` was at `0xC0000104`; per DS §4.2 `HOSC_CTL` is `0xC0000100`
  and `0xC0000104` is `LOSC_CTL`. So the port's timer start-up
  `0xC0000104 |= 1` enables the **32 kHz crystal oscillator** (LOSC_EN), not
  HOSC. New `cmu::LOSC_CTL`; `HOSC_CTL` moved.
* `gpio::mfp_bits::PULL` (`[11:8]`) removed: DS §10.4 has three separate
  pulls (bit 8 10K up, bit 9 100K down, bit 11 50K up) and bit 5 = Schmitt
  trigger (`SMIT`); new names added.
* `pmu::onoff::SHORT_PD` kept as an alias of `PRESS`: DS calls bit 1 a
  read-only live "being pressed" status, the vendor and port write 1 back as
  if it were a latch (works [HW]); INPUT-1 settles which.
* `pmu::pmuadc::lradc_en(n)` = bit 3 + n confirmed [DS] §9.3.4.1 (no longer
  [inferred]); `BAT_EN` bit 1 confirmed; bit 0 is `DC5V_EN`.
* `irq`: added `MPU` = 48, `SDMA` = 50, `dma(n)`; `UNUSED` extended to all
  "NC" lines (9, 25, 37, 38, 44, 46, 47).
* `rtc::tctl`: added `CAPTURE_SE` `[7:6]` and `LEVEL` bit 12 (T2/T3).
* All other existing addresses and bit positions matched the datasheet.

**Corrections to plan facts**

* 03 §10 "Battery: ADC enable `PMU+0x38` bit 0": bit 0 is DC5VADC_EN; the
  battery ADC is **bit 1** (reset 1). [DS]
* 03 §2.3 / system.md §1.5: SPI0 clock source codes are 0 HCLK, 1 CK24M,
  2 COREPLL, 3 DEVPLL; divider codes 0 ÷1, 1 ÷2, n ÷2n, 14 ÷1.5, 15 ÷2.5 [DS
  §4.3.3.5]. The ROM's `0x101` is therefore CK24M ÷2, not COREPLL; the port's
  `0x200` = COREPLL ÷1 = 48 MHz; the vendor's `0x303` is DEVPLL ÷6 = 30 MHz
  (its "90·d > DEVPLL" rule keeps SPI below 45 MHz). U-PWR-3 narrows to: why
  `0x101` measured ≈ 46 Mbit/s.
* Audio PLLs: by the `CMU_DACCLK` coding [DS], AudioPLL0 = 49.152 MHz
  (48 kHz family) and AudioPLL1 = 45.1584 MHz; `0xC0000118` / `0xC000011C`
  are presumably PLL0 / PLL1 [inferred].
* §7 power key: the vendor's running value `ONOFF_CTL[5:2]` = 8 decodes as
  **long press → restart after 8 s** ([5:4] = 2, [3:2] = 0), `[8:6]` = 3 = 1 s
  threshold. `VOUT_CTL[12:10]` = 4 (vendor MANAGER) is the reset default
  3.1 V, not a change.
* §6 / system.md §6: the special DMA channel at `0xC0071000` is the
  datasheet's **SDMA** (YUV/RGB LCD channel), interrupt **IRQ 50** (36 is
  EXTIRQ); its pending bits are `DMAIP` bits 10/26; `DMAIE` has no SDMA bits.
* §6 DMA: bit n + 16 of `DMAIP`/`DMAIE` is **half-transfer**, bit n complete
  [DS] (`payloads/dmatest.c` labels them the other way round). Only one of the
  ten channels moves data at a time (§5.1): pager and audio share one engine.
  `BC` is 18 bits (≤ 256 KiB − 1). UART1 RX `0x00200082` has no reload bit
  (circular only in software). SRAM2/3 DMA aliases are [HW] only (not in
  the datasheet memory map).
* §6 DMA selectors: DS names 5 "SDIO" and 6 "SD/MMC", but SD0 at
  `0xC0160000` (map: "SD/MMC") works with 5 [HW]: the pair is swapped in one
  of the two tables (U-SD-3).
* §4: UART2 `0xC00F0000`, UART3 `0xC0100000`, SPI1 `0xC00B0000`, SPI2
  `0xC00C0000`, SD1/SDIO `0xC0170000` (CLKEN0/RST0 b3, `CMU+0x18`), NAND
  `0xC0150000` [DS §3.4]. USB: RST0 b16 is pulsed first, then b15.
* §5: GPIO17 function 27 is **LRADC6** (ladder 2); `0xC01C0304` is
  **WIO1_CTL** (pin 63 "WIO1/LRADC1"), and `0x3001` there selects LRADC1 —
  it belongs to the ladder-1 row, whose "dedicated" pad is WIO1. GPIO24/25
  function 14 = TWI1_CLK/TWI1_DAT [DS §10.4]; GPIO38 function 10 = UART1_TX,
  GPIO39 = UART1_RX (SoC side of U-BT-1). GPIO48, 62 and 63 have no CTL
  register and no pad in the QFN68 list, so SWD group 0 probably cannot
  reach a pad (U-GPIO-3).
* U-GPIO-1 **answered [DS]**: `+0x200` is `GPIO_ODAT0`, the output latch; the
  output enable is bit 6 of each pin's CTL word.
* bluetooth.md §2: the UART BR value is `0x00D000D0` (disassembly), `[31:16]`
  TX divisor and `[15:0]` RX divisor = HOSC / baud [DS §7.4.5.5] (U-BT-1 baud
  half answered).
* system.md §1.5 / 03 §2.3 (LCM HOSC detour): the vendor switches to HOSC
  only around the PIO writer and GRAM read, not around the DMA blit. AHB/PIO
  strobes are timed by `EXT_CLK` in bus clocks, DMA strobes by `LCDC_CLK` in
  LCD-clock cycles [DS §9.1], so a DMA blit should not need the detour
  (U-LCM-1). The port's `LCM_RS` (`+0x10`) is `DEFAULT_COLOR`; RS is
  `LCD_CTL` bit 6. In DMA mode with `RGB565_FORMAT` = 1 a little-endian u16
  frame buffer needs no swap (U-LCM-2, to confirm by read-back).
* CMU_SD0CLK `0x41` decodes as CK24M ÷128 ÷2 = 93.75 kHz [DS], not ≈ 400 kHz.
* Audio (DS ch. 8 vs. audio.md): digital volume is 0.375 dB/step, `0xBF` =
  0 dB, `0xFF` = +24 dB, 0 = mute [DS] — so the port's `0xC0` is +0.375 dB,
  not a neutral level, and U-AUD-1 narrows to the PA gain steps and ramp
  timing. `DAC_FIFOCTL`/`ADC01_FIFOCTL`: bit 1 DRQ enable, bit 2 IRQ enable,
  `[5:4]` input select (audio.md had them shifted). `DIGCTL` bit 1 is dither.
  The FIFOs are 24-bit (`[31:8]`). A second DAC FIFO (`+0x10`, DMA selector
  0x0C) is summed into the output in hardware (`DIGCTL` bit 5) — a hardware
  path for key clicks. ADC: mic L/R and FD-buffer L/R fields are the reverse
  of audio.md; mic bias is `VMIC_CTL` (`+0x54`), never written by the vendor;
  the vendor does enable the ADC high-pass filter. The DAC supports up to
  96 kHz and the ADC 48 kHz in the datasheet; the IC-type gate is vendor
  software policy (U-AUD-2 remains a hardware try). `PA_CTL` (`+0x28`),
  `PA_ANTIPOP` (`+0x2C`) and `+0x30` are not in the datasheet ([HW]/[code]).
* For `CMU_DACCLK` bit 6 the "44.1 kHz family" reading holds only because of
  the vendor's audio-PLL values; either PLL can run at either frequency [DS].

**New unknowns** (tests in the module docs)

| Id | Question | Test | WP |
|---|---|---|---|
| U-DMA-3 | How the SDMA interrupt (IRQ 50) is enabled (no `DMAIE` bit) | SDMA transfer with NVIC 50 enabled; watch `DMAIP` bit 10 | LCD-1 |
| U-DMA-4 | "DMA0–7 can be triggered on selected events" (§5.1): no register describes it | Read-only search of vendor code | later |
| U-DMA-5 | Selectors 0x11, 0x1D (DSP) printed "-" | DSP bring-up | DSP-1 |
| U-SD-3 | Which of IRQ 17/18 and DMA selector 5/6 belongs to SD0 | SD0 read with mode `0x86` vs `0x85`; enable the SD0 interrupt and read `NVIC_ISPR` | SD-1 |
| U-SD-4 | Does `SD_STATE` DAT0 show card busy (replacing CMD13 polling) | Sample STATE after CMD24 | SD-1 |
| U-USB-13 | Registers `+0x000…0x0FC` (FOTG210 keeps OTG/EHCI registers there) | Read-only dump after init | USB-5 |
| U-SPI-1 | SPI0 `+0x14`, `+0x20…0x2C` (descrambler seed?) | Read-only dump in ROM state and after a descrambled read | UI-1 |
| U-TWI-1 | TWI SCL source (HOSC per formula; CMU+0x28 named "SPTWILK") | Time a 32-byte transfer at CLKDIV 15 against T0 | FM-1 |
| U-GPIO-2 | What `DNMOS` (bit 15) does (open drain?) | Spare pin with OUTEN+DNMOS, ODAT = 1, read IDAT | HAL-1 |
| U-GPIO-3 | SWD routing via `0xC01C0000` given that GPIO62/63 have no pad | **User check** with a probe on GPIO55/56 | later |
| U-GPIO-4 | Undocumented `+0x21C` and `+0x400` bit 6 toggled by vendor code | Read-only dump | HAL-1 |
| U-LCM-3 | Effect of `AHB_CFG_DATA` = 0 on the port's first command write | Send `0x11` in each mode, read display status | LCD-1 |
| U-PWM-1 | Does a `DUTY` write take effect while the channel runs | Change duty with and without the vendor's stop/start | LCD-1 |
| U-PWM-2 | Breath and programmable (DMA) PWM modes | Bring-up on PWM4 | later |
| U-PMU-5 | `ONOFF_CTL` bit 1: live status (DS) or latch (vendor) | Hold the key, read without clearing, release, read again | INPUT-1 |
| U-AUD-7 | `FIFOCTL.DMAWIDTH` with 16-bit DMA (vendor sets it, the port does not) | Toggle; compare level and channel order | AU-1 |
| U-AUD-8 | Hardware mixing of DAC FIFO1 (key clicks) | Second DMA stream on FIFO1 during playback | AU-1 |
| U-AUD-9 | `DAC_FIFO0_CNT` as a playback-position counter | Compare with DMA bytes played | AU-1 |
| U-AUD-11 | Recording reprograms `CMU+0x80` (the ADC clock, whose `0x33000000` bits analogue output needs) while the FM bypass or the DAC runs: does the output stay clean, at 16 / 44.1 / 48 kHz? `atj2157_hal::adc` keeps those bits and puts the DAC power-up's value back at stop, instead of the vendor's `CMU+0x80 = 0` | `rec start fm wav 20` with the radio playing, at each quality; listen for clicks, pitch changes or silence | AU-13 |
| U-AUD-12 | Which 16 of the FIFO's 24 bits the capture DMA takes with `FIFO_DMAWIDTH` = 16-bit and mode bit 20 (vendor setting): the top 16 (as playback's widening suggests) or the low ones | `rec start mic wav 10` while speaking: level in `rec status` in the −40…−6 dBFS range and clean speech mean the top 16 | AU-13 |
| U-AUD-13 | The microphone's input mode (the vendor's `CONFIG.TXT` says differential, `MIC_MIX_CTL[4]` = 0) and its gain on this board (vendor 31 dB analogue + 21 dB digital) | Speak at 30 cm: peaks near −12 dBFS; if far lower or very noisy try single-ended | AU-13 |

### Answered on the player by bring-up images, 2026-11-23

Raw numbers are in `docs/STATUS.md` §4; each item names the bring-up image
(`fw/bringup/<name>`, run with `cargo xtask dev ram <name> --test <name>`).

| Id / topic | Answer | Image |
|---|---|---|
| Interrupt masking at start | crt0 leaves BASEPRI = `0x40`; levels 1–3 stay masked until `hal::init` lowers it. Priority bytes `0x00/0x40/0x80/0xC0` work as four levels | hello |
| U-IRQ-2 | NVIC line 9 (no source) serves the software-pended RT executor at level 2 | hello |
| Timers | `CMU_TIMERnCLK = 1` (CK24M/24) gives an exact 1 MHz count: 10 s of T0 = 10,000,025 µs; T1 one-shot alarms wake on time (worst 21 µs late at 1 kHz). The port's `0xC0000104 \|= 1` is `LOSC_CTL` (32 kHz crystal enable) per HAL-0 | hello |
| U-WDT-1 (part) | Reboot = period code 0 + enable; the stage-2 mailbox `"ADFU"` then lands in USB recovery with SRAM0 intact | hello |
| U-LCM-1/2 | DMA blit with `IN_BUS = 1`, `EMDE`, `RS`, `RGB565_FORMAT = 1`, `DISP_SIZE = (h−1)<<16 \| (w−1)`, `STARTT`, DMA mode `0x9600` to `LCD_DATA`: the little-endian frame buffer goes out unchanged; no HOSC detour needed (LCD clock CK24M, `LCDC_CLK = 0`). Panel id `7C 89 F0` (ST7735S); RAMRD read-back works in PIO mode | lcd |
| Keys | Idle ladder 1 = 743, ladder 2 = 837 (±3) | keys |
| Charger | `CHG_CTL` reset state `0x8C40073A`: CHGEN 0, `CHG_VLOT` 14 (4.1875 V), `STOPV` 1 (4.18 V), `ENCHGATDT` 1. Vendor sequence with code 4 gives `0x8C450738` and `CHG_DET = 0xB8` (CC phase, current > 87.5 %) | charge |
| U-SD (no card) | With no card, CMD8 returns STATE `0xCB8`: bit 4 set without bit 0 means "no response" | sd |
| SD speed | 24 MHz data clock (CK24M), 4-bit, multi-block DMA: 9.3 MB/s with 64 KiB requests | sd |
| U-DMA-1 | The general DMA reads a PCM ring at `0x304000` through `0x18000000` | audio |
| U-DMA-2 | Reload mode re-arms automatically; the half-transfer interrupt needs `DMAIE` bit n+16 (pending bit n+16); then half/complete interrupts alternate | audio |
| DAC rate | Consumption measured against T0: 44,168 Hz for 44.1 kHz and 48,076 Hz for 48 kHz (≈ +0.15 %); open whether the audio PLL setting or the measurement is off (U-AUD-10, new) | audio |
| U-FM-1 (part) / U-ACC-1 | Software I2C with released (open-drain) lines and the 10 kΩ pad pull-ups works at 100 kHz; devices at 0x10, 0x11, 0x60 (tuner, RDA5807 id `0x5804`) and 0x27 (accelerometer, id `0x13`) | i2c |
| Tuner bands (from the vendor tables, [code], FM-1) | All three init tables set register 07h bit 9, so `BAND = 11` is 65–76 MHz (not 50–76): the Wide region is 65–108 MHz in two hardware bands. The vendor's volume write leaves 05h's low byte at 0x80 plus the volume (LNA input LNAP, LNA current field 0); FM-1 does the same | `orpheus-fm` |
| NOR | FAST READ by DMA: 175 µs per 1 KiB line at `CMU_SPI0CLK = 0x200`, 693 µs at `0x101`; 4 KiB erase + program + verify works in the free region | nor |
| PA volume / U-BT-2 (part) | `PA_CTL[14:12]` (PA volume) left at 0 makes the analogue output practically silent; 7 (vendor music value) gives full level. Measured 2026-11-24 with the Bluetooth module streaming to a host A2DP sink: 522 Hz test tone at PA 0 → digital silence, at PA 7 → RMS within 1.06 dB of the reference decode (DAC volume 0 dB). So the module's analogue tap is **after the PA** (post-amplifier, post-volume). Read registers live with `xtask dev mem` (word reads; byte reads of the DAC block return 0) | firmware |
| Bluetooth sink volume | After connect the sink's AVRCP absolute volume starts at 0 (BlueZ `MediaTransport1.Volume = 0`) until the source sends command 0x26: the stream is silent without it | firmware |
| Headphone output is direct drive | With the jack connected to a grounded input (the workstation's microphone input, sharing ground with the USB cable) the firmware failed to initialise the microSD card at boot every time; unplugged, it booted at once. The output's return is the amplifier's virtual ground (`PA_CTL` bit 6, VRO; `PA_OUTPUT_MODE = 1`), so a grounded input shorts it. **Never connect the headphone jack to equipment that shares ground with the USB host**; use a 1:1 audio isolation transformer (ground-loop isolator) or the Bluetooth capture for measurements (2026-11-24) | firmware |
| Bluetooth module HID record | Subclass 0xC0, report 1 boot keyboard (modifiers, LEDs, 3 keys, usages 0–255), report 2 consumer control (Vol±, Mute, Play/Pause, Next, Previous, Recall last, Rewind); **no mouse**; virtual cable, normally connectable, supervision timeout 5000. Read by raw SDP from the workstation 2026-11-24 | firmware |
| Bluetooth module SDP | The module (`ATS2851`, appears as "ZS285AH_TARNS") offers A2DP source+sink, AVRCP CT/TG, HFP HF/AG, **HID (0x1124)** and PnP records to a host; pairing used no PIN prompt (host agent NoInputNoOutput) | firmware |

## 10. Rules for hardware discovery work

1. One question per bring-up image or poke session; write the raw numbers into
   the log, then the conclusion here.
2. Read before write. Dump the block's registers first; keep the dump in
   `docs/hw/dumps/` (text) so the reset state is known.
3. Never write a register in the PMU regulator/charger group, the eFuse block
   or NOR sector 0 as an experiment (07 §7).
4. A test that could leave the player needing a key press says so and is
   scheduled as a user check.
5. When a vendor sequence is copied without understanding, mark it `VERBATIM`
   (02 §10 rule 8) and add an entry here.
