# Orpheus — development plan

A pure-Rust, Embassy-based firmware for the Yoton YM03 MP3 player (Actions
ATJ2157), replacing both the stock firmware's functions and the Rockbox port
in `/home/tim/Documents/GitHub/yoton/`. This is the master document: scope,
what was learned, the decisions taken, the decisions still open, and where
everything else is written down.

Plan date: 2026-11-23. Status of the work itself: [STATUS.md](STATUS.md).

## 1. Outcome wanted

A player that boots in about a second into a consistent, readable UI; plays
MP3, FLAC, Ogg Vorbis, Opus, Speex, WAV and more without a glitch while the
user browses; lasts at least as long on a charge as the stock firmware; offers
Music, Files, Radio, Bluetooth, Recorder, Pictures, Steps, Stopwatch and
Settings, plus an authenticator, a USB disk/CD emulator and a Bluetooth
presenter remote;
uses the SoC's hardware (DMA everywhere, hardware volume, PLL/voltage scaling,
standby states, the FFT/IIR engine, the hardware I2C controller) instead of
the CPU wherever the hardware can do the job; and is built so that an AI
worker can prove each step without seeing or hearing the device. It is
presented like a good open-source project: concise documentation, a website
where Orpheus runs in the browser and installs from it, one Orpheus/lyre
identity throughout.

**Project priorities (owner, 2026-11-27): "High performance SD storage and
USB mass storage should be project priorities."** Card I/O (SD timing and
bus width, multi-block DMA transfers, the file system's caching and
read-ahead, write batching) and the USB card reader / file transfer and
disk-image personalities (06 §2.4 targets: ≥ 10 MB/s sequential read,
≥ 5 MB/s write, 1 GiB integrity both ways) are first-class goals:
measured on the player (`metrics/`, `budgets.toml`), kept from regressing by
the gate, and weighed in every package that touches the card or USB. The
developer channel's file upload (≈ 15 KB/s on 2026-11-27, control
transfers) counts too.

## 2. Requirements and where they are planned

| Requirement | Where | Notes |
|---|---|---|
| Embassy-based, pure Rust, developed in this directory | 02 §4, §9 | Stable Rust; no C in firmware; boot stage 1 (already installed, 512 bytes of C) is rewritten last and optionally |
| LCD | 03 §7 | DMA blit, partial updates, 180° rotation |
| Buttons | 03 §8, 05 §3 | Ten keys, one grammar on every screen |
| PMU, power-saving and high-performance modes | 03 §2, §10; 02 §5 | Core supply 1.0 V instead of the 1.2 V default; levels 6–192 MHz; deep idle; documented S3 standby to be explored; charger with vendor values |
| "IMU" | 03 §15, 05 §7.7 | The part is a 3-axis accelerometer (no gyroscope) |
| Bluetooth | 06 §4, 05 §7.4 | Transmitter role fully; the module offers no other role (below) |
| USB controller and its modes | 06 §2–3 | Card reader, disk/CD emulation, HID keyboard (code typing), developer channel, charge-only; USB sound card (M16); host mode not planned |
| Audio DAC and input | 03 §12–13, 04 | Hardware volume, native rates, ADC with microphone/FM/line |
| FM radio | 03 §15, 05 §7.3 | RDA5807-family tuner, analogue bypass (no CPU while listening) |
| RTC | 03 §11 | Clock, alarms, wake from off |
| microSD | 03 §9, 02 §6 | FAT32 and exFAT, high-speed mode |
| Other peripherals | 03 §14–16; 01 §2 | FFT/IIR engine, hardware I2C, timers with capture, watchdog, PWM, eFuse id (read-only). AES, TRNG, Motion-JPEG and scaler blocks are listed by the datasheet without register documentation: noted in 01 §9, pursued only if cheap |
| Stopwatch, pedometer, recorder, pictures, radio, Bluetooth, music, settings, folder browser | 05 §7 | Each specified with acceptance tests |
| Boot a computer from an ISO/IMG on the card | 06 §3, 05 §7.10 | Disk and CD-ROM modes; automated boot test with QEMU |
| 2FA token | 06 §5, 05 §7.9 | TOTP/HOTP, typed over USB; honest limit on at-rest secrecy |
| Games: blocks, solitaire, 4096, a brick breaker ("breakout"-style; owner 2026-11-25), and an artillery duel against the computer or passing the player, with generated terrain, realistic wind and gravity, and useful settings for each (owner, 2026-11-25) | 05 §7.13, 08 M14 | Original implementations of the genres (own names, art, sounds) |
| Tiny text-to-speech engine, accessible UI and a book reader with read-aloud (owner, 2026-11-25) | 04 §8, 05 §7.14–7.15, 08 M15 | English only (D-14) |
| USB sound card: the player as a USB audio interface, microphone to the host, host audio to the headphone (TRS) and Bluetooth outputs (owner, 2026-11-25) | 06 §2.7, 08 M16 | UAC2 with feedback; isochronous endpoints still to be proven (U-USB-12) |
| Radio mixtapes: recording the radio moves from the Recorder to the Radio app (Menu › Record); Format, Quality and Automatic gain as in the Recorder; the capture goes to a ring buffer on the card; RDS (RadioText Plus, RadioText) gives song and artist, which split and name the tracks automatically; one copy of each song; the feel of taping the radio with a cassette deck; the recordings tagged and indexed for playback (owner, 2026-11-28) | 05 §7.3.1, 04 §5.3, 08 M7 (FM-2, UI-5e) | RDS-less stations still record, split at quiet gaps and named by station and time |
| Synth engine, MIDI playback, and a step-sequencer drum machine / synth app (owner, 2026-11-25) | 04 §9, 05 §7.16, 08 M17 | Own sound set and patches; no GPL synth code |
| Signal generator, IR remote (owner's jack IR adapter) and peripheral control through the TRS jack (owner, 2026-11-25) | 05 §7.17–7.19, 08 M18 | Output only; adapters required; output limits measured first (U-AUD-15/16) |
| Audio quality, battery, power and performance measured and optimised with USB disconnected: output into the workstation's input, battery discharging, status over Bluetooth where possible (owner, 2026-11-25) | 07 §5.11, 08 M13 | Rig decision D-12; deeper discharge D-13 |
| Bluetooth presentation remote; IMU (tilt) mouse only over Bluetooth | 05 §7.12, 06 §4.3 | **Remote in scope (owner, 2026-11-24)** over the module's HID service (keyboard + consumer reports, read 2026-11-24); BT-3 finds the command that feeds it, BT-4 builds the transport, UI-6b the app. **Tilt pointer conditional (D-10):** built only if BT-3 finds a Bluetooth transport for mouse data, otherwise dropped |
| Bluetooth audio receiver | — | **Removed from scope by the owner on 2026-11-23** (the module cannot receive audio) |
| Documentation, website, in-browser Orpheus, browser installer, branding | 09, 08 M12 | Concise docs for users and developers, open-source project files, GitHub Pages site from the repository with a WASM build of the firmware's UI/app/codec code and a WebUSB installer where the browser supports it; one Orpheus/lyre brand for firmware, docs and site (owner, 2026-11-24) |
| Ruthlessly efficient | 02 §1, §3, §10; 07 §4 | RAM and size budgets enforced by tooling; resident-set proof; profile-guided page packing |
| Polished, intuitive, consistent | 05 | Screen-level specification and golden-image tests |
| Ogg, MP3, FLAC, Speex, WAV and other useful formats | 04 §4 | Also Opus, AAC-LC, ALAC, WavPack, APE; WMA optional |
| Offload work to hardware | 02 §3.2, 03, 04 §5 | See the list in §3 below |

## 3. What the research established

Written up in `docs/hw/` (four reverse-engineering reports),
`docs/research/rust-ecosystem.md`, and consolidated in
[plan/01-hardware.md](plan/01-hardware.md).

1. **An official datasheet exists** (336 pages, register-level for most
   blocks; local copy in `docs/vendor/`). It was found late; the reports in
   `docs/hw/` predate it and are to be reconciled with it (WP HAL-0, HAL-9).
2. **Memory is the governing constraint**: 320 KiB of RAM, no memory-mapped
   flash. The Rockbox port's MPU-driven demand pager is proven and is the
   baseline. The datasheet claims hardware execute-in-place through an
   instruction cache, with no register documentation; a two-session
   investigation decides whether it can replace the pager.
3. **The core is much faster and thriftier than the Rockbox port uses it**:
   48 MHz at 1.2 V today; the vendor runs up to 144 MHz at 1.0 V and up to
   300 MHz at 1.3 V. Peripheral clocks are independent of the core clock.
4. **Hardware that can take work off the CPU**: DMA with reload mode for every
   data path (audio, card, flash, display, UART, USB has its own); 8-bit
   digital volume with ramp and a 3-bit amplifier gain; analogue bypass for FM
   and line-in; an FFT/FIR/IIR engine the stock firmware never calls; a
   hardware I2C controller on the pins the stock firmware bit-bangs; timers
   with capture; RTC alarm and several wake sources; an S3 standby state the
   stock firmware does not use. Codec arithmetic cannot be offloaded: the
   vendor's own decoders are integer code on the M4.
5. **Bluetooth is an autonomous transmitter module** ("ATS2851") on a UART
   with a small command protocol, fed analogue audio from the DAC. Sending
   music costs almost nothing. Receiving audio, HID and BLE are not in its
   command set.
6. **FM tuner**: RDA5807 family. **Motion sensor**: MiraMEMS DA2xx-family
   accelerometer. Both on GPIO24/25.
7. **Nothing charges the battery under the Rockbox port**; charger control is
   therefore an early work package.
8. **Rust ecosystem**: Embassy's released crates lag its main branch (USB
   traits changed; mass storage only on main); small allocation-free MP3,
   FLAC, Opus and SBC crates exist but are new; no pure-Rust `no_std` Vorbis,
   AAC or Speex decoder and no Bluetooth Classic host stack exist.

## 4. Decisions taken in this plan

1. **Software demand paging** (MPU, 1 KiB lines, six pool regions) as the
   execution model; interrupts masked with BASEPRI only, so page faults work
   inside critical sections; resident set proven by tooling (02 §3).
2. **Two Embassy executors**: an interrupt-level one for the audio pipeline
   and data pumps, thread mode for UI and services; one stack; no heap (02 §4).
3. **One 104 KiB mode pool** re-partitioned for playback, recording, USB
   storage, pictures (02 §1.3); full 32 KiB frame buffer kept for
   screenshot-based testing.
4. **Dual-boot NOR layout by default** (stock firmware stays bootable; Orpheus
   lives in the 480 KiB the Rockbox port uses now), the installed 512-byte
   stage 1 is kept, stage 2 is rewritten; a full-flash layout is an opt-in
   later (02 §2).
5. **Performance levels from the vendor's own table**, 1.0 V core supply,
   lowest sufficient level per workload, WFI idle, DEVPLL off unless requested
   (03 §2).
6. **Hardware-independent crates and a desktop simulator** so UI, codecs, file
   system and protocol logic are tested on the host; **golden images** for
   every screen; **hardware-in-the-loop tests over USB with no keys needed**
   (07).
7. **Decoders are written or adopted to a fixed arena**; ports from
   permissively licensed references where no suitable crate exists (04 §4).
8. **A developer channel on USB endpoint 0** in every USB personality for
   logs, screenshots, key injection and tests (06 §2.5).
9. **Bluetooth transmitter role only is committed**; whether the module can
   do HID is investigated first (06 §4.3); the Bluetooth presenter remote
   is committed (owner, 2026-11-24); the tilt pointer only if Bluetooth can
   carry mouse data (D-10). A receiver role is out of scope.
10. **Order of work**: shortest path to a daily-usable player (M5), then
    breadth (08).
11. **Last stage: verification (M11, owner's requirement 2026-11-24).**
    Before release the project's own source (dependencies excluded) reaches
    **100 % coverage** by unit tests, adversarial-input tests and fuzzing,
    with every test and fuzz target clean under **AddressSanitizer** (and
    Miri where `unsafe` code runs on the host); **every on-device function
    and every interaction of every screen and app** is tested on the player,
    also in **fuzzed key sequences** and in **several long soaks** (hours
    of interactive use with music, radio, seeking and settings changes, to
    elicit stack, buffer and paging problems); **every setting is shown to
    take effect at every value in every context it applies to (global
    options everywhere, local options in their contexts) and nowhere else**
    (VER-13, owner 2026-11-25); **every bug found by fuzzing stays in
    the test corpus** as a regression test; **memory use and performance**
    are tracked per commit as thoroughly as coverage (08 M11, 07 §3.5–3.8,
    §4.4, §5.7–5.9). Exclusions from the 100 % are an owner-reviewed list
    of provably unreachable code only.

12. **Unplugged measurement and optimisation (M13, owner's requirement
    2026-11-25).** Audio quality, battery life, power and performance are
    measured as the player is used — on battery, USB disconnected — with the
    analogue output recorded by the workstation's input, a firmware run
    engine that needs no keys and logs to the card, audio markers to align
    the recording, and an optional status channel over Bluetooth (07 §5.11).
    Optimisations are made one at a time against those numbers; the results
    become budgets. Comes after the features it measures and before M11.

13. **Games (M14, owner's requirement 2026-11-25).** A Games app with
    Blocks, Solitaire (Klondike), 4096, a brick breaker and an artillery duel (generated
    terrain, wind, gravity and air drag; against the computer or passing the
    player), each with settings, saved games and scores; the logic is a
    host-tested crate that also runs in the simulator and on the website.
    Original implementations: no names, art or levels of existing games.
14. **Speech, accessibility and books (M15, owner's requirement
    2026-11-25).** Our own small formant text-to-speech engine (rules and
    small lexicons, no GPL sources), a screen reader with which the player
    is usable with the screen off, large-text, high-contrast and
    reduced-motion settings, and a book reader (TXT, Markdown, FB2, EPUB)
    that can read aloud. M14 and M15 come after M10 and before M13 and M11.

15. **USB sound card (M16, owner's requirement 2026-11-25).** The player
    presents a USB Audio Class 2 interface (UAC1 as an option): the host's
    output plays to the headphone jack and to Bluetooth, the microphone is
    the host's input; asynchronous with a feedback endpoint so the DAC keeps
    its own clock; volume and mute agree both ways. The isochronous
    endpoints and full duplex are proven on the hardware first (UAC-0).
    After M9/M10, before M13 and M11.

16. **Synth engine, MIDI and Sequencer (M17, owner's requirement
    2026-11-25).** A shared synthesis crate (band-limited oscillators, FM,
    samples, drum models, filters, effects; host and player render the same
    samples), MIDI file playback with our own General MIDI sound set, and a
    Sequencer app (patterns of drum and synth tracks, songs, export to WAV
    and MIDI). The games' effects and the speech resonators use the same
    crate. After M10, before M13 and M11.

17. **Jack tools (M18, owner's requirement 2026-11-25).** The headphone
    output becomes a tool: a signal generator with configurable reference
    signals, an IR remote through an IR adapter in the jack (carrier made as
    anti-phase stereo; common protocols and remotes from the card), and
    peripheral control through jack adapters (gates, pulses, servo PWM,
    serial over audio, CV). A shared engine drives both channels
    sample-accurately with the music paused; the output's swing and DC
    behaviour are measured first. After M10 and M17's oscillators, before
    M13 and M11.

## 5. Decisions that are the user's

Defaults are what workers assume until told otherwise.

| Id | Decision | Default | Why it matters |
|---|---|---|---|
| D-1 | Licence of Orpheus | **Decided 2026-11-24 by the owner: MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`; the Opus and Speex ports keep Xiph's BSD-3-Clause, fonts keep theirs: `THIRD-PARTY.md`). Only permissively licensed sources may be ported; the GPL Rockbox tree is a hardware reference only | — |
| D-2 | When to give up the stock firmware for the full-flash layout | **Decided 2026-11-24 by the user: now** (stock firmware erased, full layout installed). **2026-11-27, owner: "You have my permission to change flash layout if needed, as well."** Changes to the layout inside Orpheus's area `0x001000–0x7DFFFF` (slot sizes, regions, their order) no longer need asking; sector 0 and `0x7E0000` and above stay untouchable (CLAUDE.md rule 1), backup-verify first, and a layout change goes onto the player only through `xtask dev` install paths with the old layout recoverable | Dual-boot limited the firmware to 480 KiB |
| D-3 | USB identity | pid.codes test id `1209:0001` in development; apply for a product id before release | The Rockbox port borrows Actions' vendor id |
| D-4 | Use of the 288 MHz / 1.3 V level | **Decided 2026-11-24, revised 2026-11-26 by the owner: "Dynamically clock up to 288mhz if required. Remove the manual 'performance' setting and free up any space that permits. Consider 288mhz safe, but highest power usage, and low power usage desireable."** The governor may choose 288 MHz on its own when the load needs it (lowest level that meets the demand, back down as soon as it can); Settings › System › Performance is removed (WP PWR-5). **Power mode (owner 2026-11-26): "Battery saver should peg clock at minimum, even if that reduces functionality. Performance should peg clocks at maximum. And auto should race to sleep." Refined the same day: "Battery saver should provide the lowest clocked performance level which is functional for required peripherals. Future optimization may improve responsiveness and performance of functionality, permitting. Auto should enable all functionality at minimum battery cost / maximum lifetime possible given the performance required to keep up with activities."** Datasheet check: 288 MHz is the rated maximum; VDD 1.3 V (code `0x18`) is within 0.9–1.35 V and is the hard ceiling; no per-level power figures in the datasheet (M13 measures them). | Above the reset-default core voltage; vendor-used (stock level 29, VDD code `0x18`) |
| D-5 | Open the case for a board inspection (Bluetooth chip marking, analogue tap, debug pads) | Not required | Would identify the Bluetooth chip, locate its audio tap and might expose SWD |
| D-6 | Experiments with the Bluetooth module's own firmware | **Decided 2026-11-24 by the owner: gated for now** — no module firmware reads, writes or upgrade commands; may be approved after the other development is complete (ask again then) | Recovery from a failed module upgrade is unknown |
| D-7 | Publishing `docs/hw/` and `docs/vendor/` | Kept out of git by `.gitignore` (the `yoton` repository treats vendor-derived analysis the same way) | Remove the ignore lines to publish the reverse-engineering reports; the datasheet copy should stay local |
| D-8 | Rockbox on the player | Replaced by Orpheus at milestone M2; restorable with one command | The player is the Rockbox reference until then |
| D-9 | Coverage exclusions (M11) | None; each proposed exclusion (provably unreachable code only) is listed in `coverage-exclusions.toml` with its reason and needs the owner's approval | The 100 % coverage requirement is otherwise absolute |
| D-10 | Remote transport and the tilt pointer | **Decided 2026-11-24 by the owner**: the presenter remote is a **Bluetooth** remote (keyboard and consumer reports through the module's HID service). The tilt (IMU) pointer is built **only if a Bluetooth transport can carry mouse data**; if BT-3 finds none (the module's descriptor read on 2026-11-24 has no mouse report), the pointer is dropped from the plan — no USB pointer, no module firmware work for it | Decides SENS-2 and the pointer parts of UI-6b |
| D-11 | Public repository name, GitHub Pages address, publication | **Decided 2026-11-24 by the owner: published as `timschmidt/orpheus` on GitHub, public.** Pages address follows as `timschmidt.github.io/orpheus` unless a custom domain is chosen | The site and the browser installer are publication |
| D-12 | Measurement rig for M13 | **Owner, 2026-11-25: manual cable swaps prompted by `xtask meas`; the per-port-switching hub is skipped for now** (a Genesys Logic hub is attached; whether it really cuts VBUS is untested). Capture through the workstation's rear line input (07 §5.11) | Options kept: an audio isolation transformer (ground-loop isolator) makes USB + jack safe and allows plugged/unplugged comparisons; a hub with per-port power switching (`uhubctl`) would allow unattended battery runs (audio capture then only with the transformer, since USB ground stays connected) |
| D-13 | Depth of automated discharge | **Decided 2026-11-25 by the owner: go as low as needed** — runs may continue to the low-battery warning and the firmware's own low-battery switch-off (the one exception to rule 4, for M13 discharge runs only); the player is then recharged and started by the owner | Characterises the last part of the curve, the warning thresholds and the shutdown path |
| D-14 | Speech languages (M15) | **Decided 2026-11-25 by the owner: English only** for this development effort | Other languages would each need rules, a lexicon and a native speaker's check |

Also needed from the user along the way: the checks in
[USER-CHECKS.md](USER-CHECKS.md) (listening, looking, pressing keys,
re-plugging, optional meter readings), a backup of the music card before file-
system write tests (FS-2), and a photo or description of the key legends.

## 6. Documents

| File | Content |
|---|---|
| [plan/01-hardware.md](plan/01-hardware.md) | Hardware reference, pins, interrupts, DMA plan, power facts, **unknowns register** |
| [plan/02-architecture.md](plan/02-architecture.md) | Memory budget, flash layout and boot, demand paging, tasks and priorities, power design, storage, logging, crates, coding rules |
| [plan/03-hal-drivers.md](plan/03-hal-drivers.md) | Driver-by-driver specification |
| [plan/04-audio.md](plan/04-audio.md) | Pipeline, codecs and their memory plan, DSP, recording, on-card indexes, pictures |
| [plan/05-ui-apps.md](plan/05-ui-apps.md) | UI grammar, visual language, toolkit, every screen and app |
| [plan/06-connectivity.md](plan/06-connectivity.md) | USB stack and personalities, disk/CD emulation, Bluetooth, authenticator |
| [plan/07-tooling-testing.md](plan/07-tooling-testing.md) | Host tools, simulator, conformance, hardware-in-the-loop, safety rules |
| [plan/08-milestones.md](plan/08-milestones.md) | Milestones, work packages, dependencies, risks, definition of done |
| [plan/09-docs-website.md](plan/09-docs-website.md) | Documentation set, GitHub Pages site, Orpheus in the browser (WASM), browser installer, branding |
| [STATUS.md](STATUS.md) | Living progress log (workers update it every session) |
| [USER-CHECKS.md](USER-CHECKS.md) | Checks that need a person |
| `hw/system.md`, `hw/audio.md`, `hw/bluetooth.md`, `hw/fm-i2c-imu.md` | Reverse-engineering reports from the stock firmware |
| `research/rust-ecosystem.md` | Crates, versions, Embassy state, public material on the SoC |
| `vendor/` | Datasheet text, ATJ2127 SDK register header (local reference, not redistributable) |
| `../CLAUDE.md` | Operating rules for workers |

## 7. How a worker session runs

1. Read `CLAUDE.md`, `docs/STATUS.md`, then the work package's row in 08 and
   the sections it cites. Do not read everything.
2. State the package and its acceptance in STATUS.md ("in progress").
3. Host work first; device work only through `xtask dev …` and within the
   safety rules of 07 §7.
4. Run the gate (07 §6). Record measured numbers, new hardware facts
   (01 §9 and the source document), new user checks, and anything left open.
5. Commit with the package id in the subject.

A session that ends without the gate passing leaves STATUS.md saying exactly
what fails and why.

## 8. Limits stated plainly

* The presenter remote depends on feeding the module's HID service (listed
  in its SDP record: keyboard and consumer reports; the feeding command is
  not known yet).
* The pointer exists only if a Bluetooth transport can carry mouse data
  (the module's descriptor has no mouse report; D-10). If built, it is a
  **tilt** pointer: no gyroscope, so a clutch key keeps it usable.
* The Bluetooth audio receiver was removed from scope by the owner on
  2026-11-23.
* The authenticator cannot protect secrets against someone who has the player
  and a USB cable, unless the passphrase is long: the boot ROM allows reading
  all flash.
* Several power figures (current in each state) cannot be measured by a
  worker; battery-voltage slopes are used as a proxy and the user's meter
  readings are requested where they matter.
* The hardware FFT/IIR engine, S3 standby, hardware execute-in-place, 96 kHz
  output and interrupt-type USB endpoints are documented or implied but
  unproven on this unit; each has a test and a fallback.
