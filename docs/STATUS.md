# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | Written 2026-11-23 (`docs/PLAN.md`, `docs/plan/`) |
| Code | None yet. The directory is not a git repository yet (`git init` is the first step of WP T-1) |
| Player | Was attached and running the Rockbox port from NOR (`10d6:1157`, mass-storage mode, 256 GB card with the owner's music) at 05:17 on 2026-11-23; it **left the USB bus at 06:11** (kernel log) and was not on the bus when the plan was finished. Nothing in the planning session talked to it. Cause unknown: unplugged, switched off, or battery empty (the Rockbox port does not run the charger). First device step of T-2: `xtask dev state`; if absent, ask the user to reconnect it |
| NOR | Stage 1 + stage 2 of the Rockbox port installed; vendor firmware intact; backups in `/home/tim/Documents/GitHub/yoton/dumps/` |
| Host | Fedora 43; `rustc 1.98.1` (ecosystem research saw 1.100.0 as current stable — update the toolchain pin in T-1); `thumbv7em-none-eabihf` target not installed; `arm-none-eabi-gcc`, `ffmpeg`, `sox` present; no `qemu-system-x86` check done; pyusb only in `yoton/.venv` |
| Current milestone | M0 not started |

## 2. Work packages

Status values: `todo`, `in progress (who, since)`, `done`, `done pending user
check`, `blocked (why)`.

| WP | Status | Notes |
|---|---|---|
| T-1 | todo | |
| T-2 | todo | |
| HAL-0 | todo | |
| T-4a | todo | |

(Add rows from `docs/plan/08-milestones.md` as milestones open.)

## 3. Session log

### 2026-11-23 — planning session

* Read the research repository and the Rockbox port; ran four static
  reverse-engineering studies of the stock firmware (system, audio, Bluetooth,
  FM/I2C/accelerometer) and one ecosystem study; found the vendor datasheet.
* Nothing was run on or written to the player in this session.
* Owner's decision after reading the plan: the Bluetooth audio receiver and
  the mouse (tilt pointer) are out of scope. Removed from PLAN, 02–06 and 08.
  The Remote app (USB HID presenter/media/keys) was then removed too: a
  remote that needs a cable is unattractive. WP RM-1 is deleted; USB HID is
  keyboard only (authenticator typing); Home has eleven apps. BT-3 is a
  report-only check of whether the module can do HID.
* Not done: reconciliation of `docs/hw/*.md` with the datasheet (they were
  written before it was found) — WP HAL-0 / HAL-9. The file-system/SD/UI part
  of the ecosystem study is README-level only.

## 4. Measured numbers (append; never overwrite history)

| Date | What | Value | How |
|---|---|---|---|
| (Rockbox port, 2026-11) | NOR line read, 1 KiB, SPI 48 MHz, DMA | ≈ 180 µs | FINDINGS §13 |
| (Rockbox port, 2026-11) | 320 kbit/s MP3 with UI at 48 MHz | ≈ 85 % CPU | FINDINGS §13 |
| (Rockbox port, 2026-11) | USB card-reader read speed | ≈ 2.6 MB/s | FINDINGS §13 |
| (Rockbox port, 2026-11) | Resident image / page store | ≈ 76 KiB / 48 pages | FINDINGS §13 |

## 5. Open questions for the user

See `docs/PLAN.md` §5 (decisions D-1 … D-8) and `docs/USER-CHECKS.md`.
