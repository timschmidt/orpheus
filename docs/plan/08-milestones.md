# 08 — Milestones and work packages

The order of work, what each package delivers, how it is accepted, and what
can run in parallel. Package contents are specified in documents 02–07; this
page schedules them.

## 1. How to read this

* A **work package (WP)** is sized for one worker to finish in one to three
  sessions, ending with the gate of 07 §6 and an entry in `docs/STATUS.md`.
  Size: S ≈ one session, M ≈ two or three, L ≈ split it before starting (the
  worker writes the split into STATUS.md).
* **Dev** column: `D` needs the player (only one worker may use it at a time),
  `H` is host-only (simulator, unit tests, tooling) and can run in parallel
  with anything.
* **User** column: checks only a person can make (ears, eyes, pressing keys,
  re-plugging, instruments). They are collected in `docs/USER-CHECKS.md`; a WP
  with open user checks is "done pending user check", and later packages may
  proceed unless the table says the check is blocking.
* Acceptance always includes: code + tests + docs updated (the relevant
  `docs/hw/*.md` and 01-hardware §9 when hardware facts were learned), budgets
  in `budgets.toml` updated with measured numbers.

Two tracks run side by side from the start:

* **Track D (device)**: boot, drivers, power, on-device verification.
* **Track H (host)**: tooling, simulator, UI, codecs, file system, protocol
  logic — everything that 07 §3 makes testable without the player.

## 2. Milestone overview

| M | Name | Result | Device state after |
|---|---|---|---|
| M0 | Foundations | Workspace, host tools, simulator shell, PAC from the datasheet | Rockbox untouched |
| M1 | Bring-up in RAM | Every basic peripheral proven by a RAM-loaded test image; battery charging managed | Rockbox untouched (images run from RAM) |
| M2 | Boot and paging | yotonrs boots from NOR through its own stage 2 with the pager; clock levels; HIL harness complete | **yotonrs replaces Rockbox in the free NOR region** (restorable) |
| M3 | Storage and USB link | Card + file system, settings store, USB driver with the developer channel | Live debugging over USB |
| M4 | UI foundation | Toolkit, Home, Settings, system screens on the device; RTC; power basics | Looks like a product; plays nothing yet |
| M5 | **Daily driver** | Folder-based playback (WAV, MP3, FLAC), Files, Now Playing, card reader, power states | Replaces Rockbox for everyday use |
| M6 | Library and sound | Library index, playlists, DSP, hardware engine, Vorbis, Opus, Speex, AAC and the rest | |
| M7 | Radio, recorder, sensors, tools | FM, recorder, pedometer, stopwatch/timer/alarm, pictures | Stock feature parity |
| M8 | Bluetooth | Transmitter role with remote control; investigation report on whether the module can do HID | |
| M9 | USB extras and authenticator | HID keyboard, authenticator, disk/CD emulation, (USB audio) | All requested extras that the hardware allows |
| M10 | Polish and release | Battery-life tuning, self-update, full-flash layout option, languages, manual, release | 1.0 |

M5 is the first point where the user can live with the player. Everything
before it is the shortest path there; do not pull later features forward.

## 3. Work packages

### M0 — Foundations

| WP | Content | Spec | Dev | Size | Acceptance |
|---|---|---|---|---|---|
| T-1 | Workspace, toolchain pin, lints, `yoton-proto`, `budgets.toml`, `xtask` skeleton (`build`, `size`, `test`) | 02 §9, 07 §8 | H | S | Empty firmware crate builds for `thumbv7em-none-eabihf`; gate script runs |
| T-2 | `yoton-host`: ADFU, ymon, NOR payload control, PNG; `xtask dev state/adfu/ram/peek/dump/shot/backup-verify/restore` | 07 §1–2 | D | M | Screenshot of the running Rockbox UI via ADFU; scratch-sector dump identical to `yoton.py`; `restore rockbox` rehearsed as a dry run (verify only) |
| HAL-0 | `atj2157-pac` from the datasheet: PMU, oscillators, CMU, RMU, RTC/timers/watchdog, DMA, SPI, UART, TWI, DAC, ADC, LCD, PWM, LRADC, GPIO; SD and USB from the port's headers and `usb_controller.md` | 03 §1, 01 §1 | H | M | Register addresses and the fields the Rockbox port uses match `atj2157.h`; every field tagged with its source |
| T-4a | Simulator shell: window, headless frame buffer, key scripts, golden harness with one dummy screen | 07 §3.2 | H | S | `xtask test golden` passes and fails correctly on a changed pixel |

### M1 — Bring-up in RAM (Rockbox stays installed)

All images are RAM-only (< 96 KiB), loaded by `xtask dev ram`.

| WP | Content | Spec | Dev | Size | Acceptance | User |
|---|---|---|---|---|---|---|
| HAL-1 | crt0, SRAM2/3, priorities, BASEPRI critical section, both executors, timers and time driver, watchdog, GPIO; unknowns U-CPU-1/2, U-IRQ-2, U-WDT-1 | 03 §3–4 | D | M | `hello` image: timer accuracy vs host; tasks on both executors; clean return to ADFU | — |
| T-5 | HIL harness: test mode, result block, fault record, log ring over ADFU, suite runner | 07 §5 | D | M | `hil smoke` on the `hello` image incl. a deliberate fault test that produces a decoded fault record | — |
| PWR-2 | Battery ADC, cable detect, **charger state machine with vendor values**, charge state reporting. Early because nothing charges the battery while non-stock firmware runs | 03 §10 | D | M | Charger enters and leaves states on a scripted host model; on the device `CHG_DET` shows charging at the 200 mA code when plugged | USB meter reading if available; full charge terminates |
| LCD-1 | LCM, panel, backlight, DMA blit, read-back; U-LCM-1/2 | 03 §7 | D | M | Panel read-back equals the frame buffer; screenshot path works | Picture looks right (colours, orientation, no tearing) |
| INPUT-1 | Keys, power key, earphone sense, injected events | 03 §8 | D | S | Trace-based tests; live raw-value screen | Press each key once: all ten register correctly — **blocking for M4** |
| SD-1 | SD host, protocol layer, block device | 03 §9 | D | M | Pattern test in the scratch file region; card info matches the host's view | — |
| AU-0 | DAC bring-up port: power-up, tone by DMA, hardware volume and mute, power-down; U-DMA-1/2, U-AUD-1/2 | 03 §12 | D | M | DMA cadence correct at 44.1 and 48 kHz; reload mode characterised | 1 kHz tone on left then right; no loud pop at start/stop; volume steps even — **blocking for M5** |
| FM-0 | I2C scan (software master, then TWI1), tuner and accelerometer ids; U-FM-1, U-ACC-1 | 03 §15 | D | S | Devices found at `0x10` and `0x27`; ids recorded | — |

### M2 — Boot and paging (takes over the free NOR region)

| WP | Content | Spec | Dev | Size | Acceptance |
|---|---|---|---|---|---|
| T-3 | `xtask pagelink`, `resident.toml`, `hot.toml`, `check-resident`, `PagerCore` host tests | 02 §3, 07 §4.3 | H | M | Toy firmware splits into resident + pages; simulated-MPU trace tests pass; `check-resident` catches a planted violation |
| PG-1 | `NorCore`, pager on device, slot format, stage 2 in Rust, crash screen, boot counter; U-NOR-1, U-PWR-3 | 02 §2–3, 03 §6 | D | L | Boots from NOR without keys ten times; `hil pager`; `restore rockbox` verified for real once, then yotonrs re-staged |
| PWR-1 | Clock governor: 1.0 V core supply, levels `Low`…`High` (then `Turbo`), self-test, statistics | 03 §2 | D | M | 03 §2.4 acceptance |
| XIP-1 | Time-boxed look at the hardware SPI cache (two sessions maximum) | 01 U-XIP-1 | D | S | A written verdict in 01-hardware: works (with a design note for replacing the pager) or does not (with what was tried) |
| T-8 | Profilers (PC sampling, fault log), `hot.toml` loop | 07 §5.6 | D | S | Profile of the demo UI; regenerated `hot.toml` reduces faults measurably |

After PG-1 the worker asks nothing of the user, but notes in STATUS.md that
the Rockbox port is no longer installed and how to bring it back.

### M3 — Storage and USB link

| WP | Content | Spec | Dev | Size | Acceptance |
|---|---|---|---|---|---|
| FS-1 | File system decision and implementation: FAT12/16/32 + exFAT, long names, extents, write support | 02 §6, 07 §3.1 | H | L | Host suite incl. `fsck` and power-cut tests |
| FS-2 | File system on the device: mount the owner's card read-only first, then writes inside `/.yoton/` | 02 §6 | D | S | `hil fs`; directory listing equals the host's (compare via card-reader mode later, via known files now) |
| PG-2 | NOR writes with suspend, settings store; U-NOR-2 | 03 §6 | D | M | 03 §6 acceptance |
| USB-1 | USB controller driver, vendor-only device, first developer-channel requests; U-USB questions | 06 §2.1–2.2, §7 | D | L | Enumerates at high speed 100/100 times; control transfers of every size class correct |
| USB-2 | Developer channel complete, host side, `xtask dev log/shot/keys/stats/test/time` live | 06 §2.5 | D | M | `hil smoke` runs in live mode in under 10 s |
| SD-2 | High-speed card mode with DEVPLL; U-SD-1 | 03 §9 | D | S | Speed and integrity targets of 03 §9 |

### M4 — UI foundation

| WP | Content | Spec | Dev | Size | Acceptance | User |
|---|---|---|---|---|---|---|
| UI-1 | Toolkit core, fonts (incl. fallback-font resource and U-NOR-3), widgets | 05 §4–5, 02 §7 | H | L | Golden images for every widget in two themes and three scripts | Photo or description of the key legends (05 §1) |
| UI-2 | Home, Settings (all groups, backed by the settings service), system screens: boot, charging, power menu, lock, quick panel, errors | 05 §6, §7.11 | H | L | Golden journeys |
| SYS-1 | Device integration: `ui`, `input`, `display`, `settings`, `power`, `storage` tasks wired; services implemented; watchdog policy | 02 §4 | D | M | `hil smoke` screenshot of Home equals the simulator's golden image exactly; key script walks every Settings group on the device |
| RTC-1 | RTC, alarms, time setting UI hooks; U-RTC-1 | 03 §11 | D | S | 03 §11 acceptance | — |
| PWR-3a | Shutdown path, power-off, wake enables, auto-off, backlight and lock timers, low-battery handling; U-PMU-3 (alarm wake) | 03 §10, 02 §5 | D | M | Alarm-wake test automatic; state saved and restored across power-off | Power off and on by key; plug-in wake |

### M5 — Daily driver

| WP | Content | Spec | Dev | Size | Acceptance | User |
|---|---|---|---|---|---|---|
| T-7 | Codec corpus generator, conformance harness, fuzz targets | 07 §3.3–3.4 | H | M | Harness validates a reference WAV path end to end |
| AU-1 | Pipeline core, pool, WAV family, DAC driver complete (with AU-0 results), underrun accounting | 04 §2–3, §8 | H+D | L | Host timing simulation; `hil audio` CRC for WAV at all native rates; zero underruns under UI stress |
| AU-2 | MP3 | 04 §4 | H+D | M | Conformance, fuzz, benchmark row, CRC on device |
| AU-3 | FLAC | 04 §4 | H+D | M | Bit-exact + MD5; benchmark |
| AU-4 | Metadata, album art, lyrics parsing | 04 §4.4 | H | M | Parser tests and fuzzing |
| AU-5 | Playback service: queue, locators, shuffle, repeat, resume, bookmarks | 04 §2.2, §6.3, §6.5 | H | M | Service tests incl. simulated power loss |
| UI-3 | Files app, directory index (first half of AU-12), text viewer | 05 §7.2, 04 §6.2 | H+D | M | Golden journeys; 5000-entry directory targets on the device |
| UI-4 | Music app (Folders, Shuffle all, Now Playing with all views, equaliser screen shell) | 05 §7.1 | H+D | L | Golden journeys; `hil audio` with UI scripts | Listening test: MP3 and FLAC albums, gapless album, seek, resume — **gate for M5** |
| USB-3 | Mass storage (card reader) + pump | 06 §2.4 | D | M | Integrity and speed targets |
| USB-4 | Personality manager, USB dialog, charge-only, charger/host detection | 06 §2.3, 05 §6.5 | D | M | Scripted switches between personalities without re-plugging |
| PWR-3b | Screen-off playback policy, deep idle, per-driver gating; U-PWR-4 | 02 §5, 03 §10 | D | M | `hil power`; battery-slope comparison against the Rockbox port's figures if recorded | Current measurements table (optional) |
| REL-0 | "Daily driver" soak and fix week: 2 h soak suite, bug list from the user's real use | 07 §5.4 | D | M | Soak passes three times in a row |

### M6 — Library and sound

| WP | Content | Spec | Dev | Size |
|---|---|---|---|---|
| AU-12 | Library index, tag-based browsing, playlists (`.m3u/.m3u8`), Favourites | 04 §6, 05 §7.1 | H+D | L |
| AU-6 | DSP stages in software: ReplayGain, EQ, bass, crossfeed, balance, limiter, decimator, dither, meters, then speed | 04 §5.1 | H+D | L |
| DSP-1 | Hardware FFT/IIR engine: investigate, characterise, integrate behind the DSP traits | 03 §14 | D | M |
| AU-7 | Ogg container + Vorbis (low-memory decoder) | 04 §4 | H+D | L |
| AU-8 | Opus | 04 §4 | H+D | L |
| AU-9 | Speex | 04 §4 | H+D | M |
| AU-10 | MP4 container, AAC-LC, ALAC | 04 §4 | H+D | L |
| AU-11 | WavPack, APE | 04 §4 | H+D | M |
| UI-4b | Music app completion: tag lists, playlists, lyrics view, art, queue editing, levels view | 05 §7.1 | H+D | M |

Each codec package has the acceptance list at the end of 04 §8. User check
per codec: one listening pass with real files of that format.

### M7 — Radio, recorder, sensors, tools

| WP | Content | Spec | Dev | Size | User |
|---|---|---|---|---|---|
| FM-1 | I2C driver final form, tuner driver, analogue route, volume; U-FM-2/3; RDS if available | 03 §15 | D | M | Reception on local stations; stereo; volume |
| UI-5a | Radio app, presets, auto-scan, regions | 05 §7.3 | H+D | M | |
| AU-13 | ADC driver, capture path, WAV/ADPCM/FLAC/voice encoders, AGC, VOX, write-behind; U-AUD-4/6 | 03 §13, 04 §5.2 | H+D | L | Listen to recordings from each source |
| UI-5b | Recorder app | 05 §7.5 | H+D | M | |
| SENS-1 | Accelerometer driver, step detector, background counting; trace capture tool | 03 §15, 05 §7.7 | H+D | M | Walk 200 counted steps twice; table-rest test |
| UI-5c | Steps app, history | 05 §7.7 | H | S | |
| UI-5d | Stopwatch, Timer, Alarm (with RTC alarms and the standby decision) | 05 §7.8 | H+D | M | Alarm wakes the player |
| PWR-3c | S3 standby investigation and integration (U-PMU-2); else deep-idle fallback for alarms and step counting | 03 §10 | D | M | |
| AU-14 | Picture decoders, scaler, thumbnails; Pictures app; wallpaper | 04 §7, 05 §7.6 | H+D | L | Look at a set of photos |

### M8 — Bluetooth

| WP | Content | Spec | Dev | Size | User |
|---|---|---|---|---|---|
| BT-1 | UART1 driver, pin assignment test (U-BT-1), frame codec, command layer, identity | 06 §4.2 | H+D | M | — |
| BT-2 | Transmitter role: state machine, pairing, AVRCP bridge, volume policy, reconnect, power | 06 §4.2 | H+D | L | Pair a headset and a speaker; music, keys, volume, reconnect; note whether the wired output stays live (U-BT-2) — **gate** |
| UI-6a | Bluetooth app and quick-panel rows | 05 §7.4 | H | M | |
| BT-3 | Investigation: can the module do HID (Bluetooth remote, code typing)? Report only; nothing built without a new owner decision | 06 §4.3 | D | S | Phone scan observations; optional chip marking (decision D-5) |

### M9 — USB extras and authenticator

| WP | Content | Spec | Dev | Size | User |
|---|---|---|---|---|---|
| USB-5 | HID keyboard interface; U-USB-11 | 06 §2.3, §5 | D | S–M | — |
| OTP-1 | `yoton-otp`, vault, PIN flow, time sync tool | 06 §5 | H+D | M | — |
| OTP-2 | Authenticator app, typing, provisioning, `TextEntry`/PIN pad | 05 §7.9 | H+D | M | Log in to a test account with a typed code |
| USB-6 | Disk and CD emulation, USB Drive app, QEMU boot test | 06 §3, 05 §7.10 | H+D | L | Boot one real PC from an ISO (if no QEMU pass-through) |
| USB-7 | USB audio (stretch; U-USB-12) | 06 §2.3 | D | M | Listen |

### M10 — Polish and release

| WP | Content | Spec | Dev | Size |
|---|---|---|---|---|
| UI-7 | Icons/themes pass, animations, rotation, wallpaper, screen saver, language packs, on-device acceptance sweep with archived screenshots | 05 §9 | H+D | L |
| PWR-4 | Battery-life campaign: scenario table (idle, MP3/FLAC screen off, radio, Bluetooth, recording), tuning of levels, gating, read-ahead; comparison with stock | 02 §5 | D | M |
| UPD-1 | Firmware file format, RAM installer, Update screen, slot handling | 02 §2.5 | D | M |
| LAY-1 | Full-flash layout option, Unifont pack, `install-full`, `restore vendor` (decision D-2) | 02 §2.4 | D | M |
| T-6 | Rust payloads (ymon, NOR tools), optional Rust stage 1 | 07 §8 | D | S |
| AU-15 | Optional formats (WMA, trackers, CUE) | 04 §4 | H+D | M |
| DOC-1 | User manual (one page per app), developer guide, hardware notes cleaned up, release notes | — | H | M |
| REL-1 | Release checklist: all gates, all HIL suites, soak ×3, all user checks ticked or waived by the user, tagged build for each layout | 07 §6 | D | S |

## 4. Dependencies that matter

```
T-1 ─┬─ T-2 ── HAL-1 ─┬─ T-5 ── (every D package)
     │                ├─ LCD-1 ─┐
     │                ├─ INPUT-1├─ SYS-1 ── UI on device
     │                ├─ SD-1 ── FS-2 ── AU-1(device) ── codecs
     │                ├─ AU-0 ───────────┘
     │                └─ PWR-2
     ├─ HAL-0 (feeds every driver)
     ├─ T-3 ── PG-1 ─┬─ PWR-1
     │               ├─ PG-2 ── settings ── SYS-1
     │               └─ USB-1 ── USB-2 ── live HIL (speeds up everything after)
     └─ T-4a ── UI-1 ── UI-2 … (host track, limited only by service traits in yoton-apps)
FS-1, T-7, AU-4, AU-5, codec host work: host track, any time after T-1
```

Rules of thumb for whoever assigns sessions:

* Keep one worker on Track D continuously; hardware knowledge accumulates in
  STATUS.md and 01-hardware §9.
* Track H packages can be handed to parallel workers freely; they meet Track D
  at SYS-1 (M4) and AU-1 (M5).
* USB-1/USB-2 are scheduled early (M3) on purpose: live log, screenshot and
  key injection cut every later device cycle from minutes to seconds.
* Never start a package whose acceptance needs an unanswered **blocking**
  user check.

## 5. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Resident set outgrows 72 KiB (Embassy core + USB + audio ISR paths) | Medium | RAM for buffers shrinks | Size gate from T-1; hand-written resident paths; move USB class code out of the resident set (only the IRQ shim stays) |
| Page-fault working sets thrash with Rust code density | Medium | CPU and power | Profile-guided packing (T-8); hot codec loops measured early (AU-2); the bank-tag and hardware-cache options (U-MEM-1, U-XIP-1) as reserves |
| A decoder does not fit its arena or the CPU budget (Vorbis, Opus most likely) | Medium | Format dropped or limited | Arena-first design, low-memory algorithms, higher clock levels now known to be available at 1.0–1.1 V; refuse oversized streams cleanly |
| NOR suspend/resume unsupported | Low–medium | Settings writes disturb audio | Deferred-write fallback (02 §3.2 rule 5) |
| Datasheet and silicon disagree (the vendor driver gates features by IC type) | Medium | Lost time | Bring-up image per driver; read-before-write; trust tags |
| USB controller surprises with interrupt/isochronous endpoints | Medium | HID or USB audio delayed | Both are after M5; mass storage path is already proven by the port |
| Bluetooth module pin/level assumptions wrong | Low | BT delayed | Identity test both ways round first; module is self-contained |
| Player needs a human after a bad state (power-off, hang) | Certain, occasionally | Stalls unattended work | Watchdog + boot counter + ADFU; hardware 8 s long-press reset; no automated power-off; worker stops and asks rather than retrying blind |
| Card data damaged by a file-system bug | Low | Severe for the owner | Read-only mount first; writes confined to `/.yoton/`; host `fsck` after every write suite; the owner should keep a backup of the card (ask once, at FS-2) |
| Battery harmed by charger misuse | Low | Severe | Vendor values only, documented currents, early PWR-2, user meter check |
| Third-party crates immature | High | Rework | Each adopted only through our tests; ports as fallback are already planned |
| Scope | High | Never finishing | M5 gate first; everything after is ordered by user value; stretch items marked |

## 6. Definition of done for the whole project

* Every requirement row in PLAN.md §2 is delivered or closed with a written
  hardware reason.
* Gates green on both layouts; HIL suites `smoke`, `drivers`, `pager`,
  `audio`, `fs`, `usb`, `power`, `soak` pass on the reference player.
* Battery life at least equal to stock in the PWR-4 scenario table, or the gap
  explained.
* No C in the tree except vendored test references; firmware crates build on
  stable Rust.
* The user has used the player daily for two weeks on a release candidate
  without a fault record.
