# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | `docs/PLAN.md`, `docs/plan/` (scope changes of 2026-11-23 applied) |
| Code | Git repository since 2026-11-23. Host workspace (`crates/*`, `xtask`) and device workspace (`fw/`: `atj2157-hal`, `yoton-bsp`, `firmware`, `bringup/*`) |
| Player | Attached; Rockbox port installed in NOR and untouched; all yotonrs code so far runs as RAM images through ymon. **No microSD card answers** (Rockbox reports "Media removed" too): SD-1 on-device tests and everything after FS-2 wait for a card |
| NOR | Stage 1 + C stage 2 + Rockbox. Vendor backups verified (`ad45687e…`). The whole free region `0x762000–0x7DFFFF` as installed is saved in `local/rockbox-nor-0x762000.bin` (sha256 in the `.sha256` next to it); `cargo xtask dev restore rockbox --verify` matches |
| Host | Fedora 43, Rust 1.98.1 pinned (`rust-toolchain.toml`), `thumbv7em-none-eabihf` installed |
| Current milestone | M1 (bring-up in RAM) nearly complete; M0 done except T-4a (worker running) |

## 2. Work packages

| WP | Status | Notes |
|---|---|---|
| T-1 | done | Workspaces, toolchain pin, lints, `yoton-proto`, `budgets.toml`, `xtask build/size/test/gate` |
| T-2 | done | `yoton-host` (nusb BOT, ADFU/ymon, SG_IO reboot, NOR payloads, PNG); `xtask dev state/adfu/ram/results/peek/poke/dump/shot/nordump/backup-verify/save-rockbox/restore rockbox [--verify]/reset`. Rockbox screenshot, NOR dump identical to `yoton.py`, restore dry run verified. Still uses the reference repo's C `ymon.bin`/`nordump.bin`/`norprog.bin` (T-6 replaces them) |
| HAL-0 | done | `atj2157-pac` complete (worker); corrections in `docs/plan/01-hardware.md` §9 |
| T-4a | in progress (worker) | Simulator + golden harness, with UI-1 |
| HAL-1 | done | crt0, vectors + IRQ dispatch, fault record, BASEPRI critical section, priorities, T0/T1 time driver at 1 MHz, main + RT executors, watchdog, GPIO |
| T-5 | done (first part) | `xtask dev hil smoke` / `drivers`; result block, fault record, log ring, mailbox; gate runs `hil smoke` |
| LCD-1 | done pending user check 7 | LCM PIO + DMA blit, ST7735S panel, backlight PWM4, GRAM read-back |
| INPUT-1 | done pending user check 8 (**blocking M4**) | `yoton-input` decoder/debouncer; live key screen |
| PWR-2 | done pending user check 9 + full-charge termination | `yoton-power` vendor charge algorithm, `yoton-bsp::charger`; charges at code 4 (200 mA) |
| SD-1 | blocked (no card in the slot) | Driver and `yoton-sdcard` done; read test reports SKIP until a card is inserted |
| AU-0 | done pending user check 10 (**blocking M5**) | DAC power-up, chained and reload DMA, SRAM3 ring, volume |
| FM-0 | done | Soft I2C master; tuner and accelerometer ids |
| FS-1 | in progress (worker) | `yoton-fs` |
| T-7, AU-1 (host), AU-2, AU-3, AU-4 | in progress (worker) | audio core and codecs |

## 3. Session log

### 2026-11-23 — implementation session 1 (lead + four host-track workers)

* M0 tooling and M1 bring-up images on the player (all RAM-loaded; Rockbox
  untouched in NOR). Each bring-up image is a HIL case:
  `cargo xtask dev hil smoke` (hello, deliberate fault) and
  `cargo xtask dev hil drivers` (lcd, keys, charge, sd, audio, i2c).
* Hardware facts learned (also for 01-hardware §9):
  * BASEPRI must be lowered after init: start-up masks levels 1–3.
  * Timers clocked at CK24M/24 give an exact 1 MHz tick (10 s window =
    10,000,025 µs vs host); RT ticker at 1 kHz: worst lateness 14–21 µs.
  * Panel id `7C 89 F0` (ST7735S); DMA blit with `IN_BUS=1`,
    `RGB565_FORMAT=1` streams the little-endian frame buffer unchanged:
    2.77 ms per full screen at LCD clock CK24M with 1+1 strobe clocks; GRAM
    read-back (RAMRD) works.
  * Key ladders idle at 743 / 837 (noise ±3).
  * Charger: `CHG_CTL` reset state `0x8C40073A` (off, CV 4.1875 V, stop
    4.18 V); charging `0x8C450738` (code 4); `CHG_DET` `0xB8` = CC phase.
  * DMA: half-transfer interrupt needs `DMAIE` bit n+16; reload mode works
    (U-DMA-2); DMA reads SRAM3 through `0x18000000` (U-DMA-1).
  * DAC sample clock measured ≈ +0.15 % fast at 44.1 and 48 kHz against
    T0 (open: audio PLL value vs measurement).
  * I2C: devices at 0x10/0x11 (tuner, also 0x60), 0x27 (accelerometer);
    tuner id 0x5804, accelerometer id 0x13; lines idle high.
  * SD: no card answers (CMD8 "no response", STATE bit 4).
  * Stack overflow is silent without the MPU guard (a 64 KiB temporary on
    the stack jumped to address 0): PG-1 must add the guard region.
* Scope changes from the owner, applied to the plan: no Bluetooth receiver,
  no mouse, no Remote app.

### 2026-11-23 — planning session

* Read the research repository and the Rockbox port; four static
  reverse-engineering studies of the stock firmware and one ecosystem study;
  found the vendor datasheet. Nothing was run on the player.

## 4. Measured numbers (append; never overwrite history)

| Date | What | Value | How |
|---|---|---|---|
| (Rockbox port, 2026-11) | NOR line read, 1 KiB, SPI 48 MHz, DMA | ≈ 180 µs | FINDINGS §13 |
| (Rockbox port, 2026-11) | 320 kbit/s MP3 with UI at 48 MHz | ≈ 85 % CPU | FINDINGS §13 |
| (Rockbox port, 2026-11) | USB card-reader read speed | ≈ 2.6 MB/s | FINDINGS §13 |
| (Rockbox port, 2026-11) | Resident image / page store | ≈ 76 KiB / 48 pages | FINDINGS §13 |
| 2026-11-23 | T0 window vs host | 10,000,025 µs in ≈ 10.0 s | `hil smoke/hello` |
| 2026-11-23 | RT executor 1 kHz ticker lateness | max 14–21 µs, mean 14 µs | `hil smoke/hello` |
| 2026-11-23 | Main executor idle share at rest | 99.9 % | `hil smoke/hello` |
| 2026-11-23 | LCD full-screen DMA blit / PIO fill | 2.77 ms / 6.85 ms | `hil drivers/lcd` |
| 2026-11-23 | DAC consumption at 44.1 / 48 kHz | 44,168–44,229 / 48,076 Hz | `hil drivers/audio` |
| 2026-11-23 | Battery raw at rest / charging | 801 / 828 (≈ 3.74 V) | `hil drivers/charge` |

## 5. Open questions for the user

* **Insert the microSD card** (any card; contents are only read until the
  file-system package confines writes to `/.yoton/`).
* User checks 7–10 in `docs/USER-CHECKS.md` (picture, keys, charge current,
  tones). 8 and 10 are blocking for M4 and M5.
* Decisions D-1 … D-8 in `docs/PLAN.md` §5 (defaults assumed).
