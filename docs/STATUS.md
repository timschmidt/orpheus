# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | `docs/PLAN.md`, `docs/plan/`. **D-2 decided 2026-11-24 by the user: full-flash layout** ("wipe the original firmware and take over the full player hardware") |
| Code | Host workspace (`crates/*`, `sim`, `xtask`) and device workspace (`fw/`: `atj2157-hal`, `yoton-bsp`, `firmware`, `boot/stage2`, `payloads/norprog`, `bringup/*`) |
| Player | **Runs Orpheus from NOR, full layout.** Stock firmware and the Rockbox port are erased. Card: the owner's music (exFAT/FAT, 238.3 GB), read only except `/.orpheus/` |
| NOR | Sector 0: Rockbox-port stage 1 (unchanged). `0x001000`: full slot A (Orpheus, 544,768 B). `0x7DA000` settings, `0x7DE000` vault, `0x7DF400` Rust stage 2 (876 B, tries full A, full B, dual slot; no stock-firmware path). `0x7E0000–0x7FFFFF` vendor data, untouched. Backups: `yoton/dumps/nor_backup_8m.bin` (stock, verified), `local/nor-before-full.bin` (the chip just before the switch), `local/rockbox-nor-0x762000.bin`, `local/stage2-previous.bin`. Way back: `xtask dev restore vendor --i-have-a-backup` (`--verify` compares) |
| Host | Fedora 43, Rust 1.98.1 pinned. Root disk nearly full (≈ 14 GB free after deleting worker build output) |
| Current milestone | M1–M3 device parts done; M4 (Files/Music) and M5 (playback) running on the player; host side of M6–M8 packages largely done |

## 2. Work packages

| WP | Status | Notes |
|---|---|---|
| T-1, T-2, T-4a, HAL-0, HAL-1, T-5 | done | see earlier log entries |
| T-3, PG-1 | done | pagelink, PagerCore, check-resident; demand paging on the player (8 KiB pages, 1 KiB lines) |
| T-6 | started | Rust NOR payload `fw/payloads/norprog` (erase/program, full-layout guard); ymon and nordump still the reference repo's C payloads |
| LAY-1 | done (device) | Full layout installed (`xtask dev install-full --i-have-a-backup`), layout-aware `stage`, `restore vendor [--verify]`. Unifont pack in NOR not done (the UI's fonts are built in) |
| LCD-1, INPUT-1, PWR-2, AU-0 | done pending user checks 7, 8, 9, 10 | |
| SD-1, FM-0, USB-1, USB-3, PWR-1 | done | |
| SYS-1 | done (first cut) | Firmware: tasks, settings in NOR, developer channel, watchdog task fed only on RT + main + UI progress (verified: a hung main executor resets in ≈ 3 s), test safety window |
| PWR-2 governor | first cut | 48 ↔ 96 ↔ 144 MHz from main-executor idle share; decision now `orpheus_power::governor` (host-tested) |
| PWR-5 (D-4) | Max level verified on the player; **Performance setting, battery logger, `xtask drain report` built (not yet run on the player)** | Settings › System › Performance (Normal/Maximum: the governor's step above High is 288 MHz); battery logger to `/.orpheus/test/drain-<n>-<label>.csv` (`drain start/stop/status`, hidden System › Developer row); user check 19 (unplugged drain runs) |
| PWR-3a | mostly done (device) | Power-off: card flushed, Bluetooth 0x24, screen off, vendor PMU sequence (wake on long press or USB plug-in); in test mode a reboot instead (verified). Auto-off (setting, no key, nothing playing, no cable) and low-battery off (< 3.30 V for 10 s, no cable) through the same path — not testable with the cable in: user checks 11 and 15. Alarm wake not done |
| RTC-1 | done (device) | set/read over the developer channel (`xtask dev time [--set]`); alarm untested; once lost after a ymon reset (U-RTC open) |
| USB-4 | done (device) | Card-reader personality: card handed between the card task and USB; write-protected test mode; mid-track switch and hand-back verified. 2.3 MB/s read (open) |
| FS-1 | done | `orpheus-fs` |
| FS-2 / AU-5 firmware | first cut on the player; **`orpheus-player` wired in; gapless hand-over and queue edits (built, not yet run on the player)** | Files lists the card (sorted through the directory index since AU-12 firmware), text viewer window, Now Playing. The card task drives `orpheus_player::Player`: folder queue in RAM and `/.orpheus/queue.bin`, resume record and bookmarks in `/.orpheus/player.bin`, seek, shuffle/repeat (settings), A–B, sleep timer, Up next. Gapless read-ahead (`orpheus_audio::StreamRing`) and decoder re-open with the DAC running; Up next remove/move, Play next, Add to queue (256 max). Not done: fades. User checks 13, 17 |
| UI-1, UI-2, UI-3, UI-4 | done (host); running on the player | 457 golden images |
| AU-1…AU-4, AU-7, AU-8, AU-9 | done (host); on the player: MP3/FLAC/WAV and Speex at 48 MHz, Vorbis at 144 MHz (clock floor), **Opus too slow (fix built, not yet run on the player)** | Opus fix (AU-8, 2026-11-24): Ogg pages above 8 KiB read packet by packet (16 KiB ring); Opus frame code and tables packed in `hot.toml` clusters `opus-celt`/`opus-frame`/`opus-silk` and compacted: decoder alone 61.5 → 0 NOR line loads per CELT frame (emulator + pager replay). Hybrid (SILK + CELT) still pages (~17–52 loads per frame). `status` shows `dec:` (decoder µs and line loads per track) |
| AU-6 | done (host); **working on the player** | `orpheus-dsp` chain between decoder and DAC, fed by the Sound settings; works on a scratch copy of the decoder output; while any stage is active during playback the clock floor is 96 MHz (EQ + bass + crossfeed: 0 underruns, ≈ 90 % idle). WSOLA not done; Levels meter not wired |
| AU-12 (directory index, music library) | host done; **firmware wired in (built, not yet run on the player)** | `orpheus-index` + its `card` adapters over `orpheus-fs` (host-tested on FAT32/exFAT images). Files/Music › Folders sorted (Name/Date/Size, hidden per setting), letter jump from the index's letter runs, play queue in listing order. **Music library** (`orpheus-library`) built in the card task's idle loop, Music › Artists/Albums/Songs/Genres/Recently added, Play/Shuffle, Shuffle all from the library, Go to album/artist. Playlists, Favourites: not started. User check 16 |
| AU-14 | done (host); **device: Pictures app and album art (built, not yet run on the player)** | `orpheus-img`. Pictures (folders + pictures, list mode), viewer (fit/fill, rotate, next/prev, slideshow), album art made at track open and cached in `/.orpheus/cache/art`. Not done: thumbnail grid, 100 % + pan, wallpaper, delete, per-file rotation memory. User check 14 |
| OTP-1, BT-1, USB-6 | done (host) | `orpheus-otp`, `orpheus-bt`, MMC/CD-ROM in `orpheus-usb` |
| BT-2 (device) | **built, not yet run on the player** | UART1 driver (`atj2157_hal::uart`, `yoton_bsp::bt`), `orpheus_bt::driver`, firmware `bt` task, `BluetoothService`, shutdown 0x24 on power-off. Module off at boot; `bt probe/on/off/status/scan/connect/listen/shutdown/rx/tx` test commands. Not done: volume policy, metadata at track change, pairing UI, U-BT-2 |
| SENS-1 | **built, not yet run on the player** | `orpheus-steps` (detector, history, trace format; synthetic-trace tests), `yoton_bsp::accel` (no FIFO: part unidentified, U-ACC-1), `firmware::sensor` task (25 Hz, SRAM3 state), `/.orpheus/steps.bin`, `acc read/regs/trace`, `steps status`. User check 20 |
| UI-5c | **built (host + firmware), not yet run on the player** | Steps app (Today/Week, goal, distance/kcal, settings, reset), 73 goldens (UI-5d: 98) |
| UI-5d | **built (host + firmware), not yet run on the player** | Stopwatch/Timer/Alarm in the background, alert screen, beep, backlight flash, status-bar bell/dot, RTC alarm armed; wake from off waits for PWR-3c (`ALARM_WAKE_FROM_OFF = false`). User check 21 |
| UI-6a | **on the player 2026-11-24** (connect, reconnect, audio, volume — policy changed, see session entry) | Bluetooth app (status, Pair new device, Paired devices, connect progress/failure, forget, reconnect switch), `BluetoothService` lists/requests, volume policy of 06 §4.2 (0x26 on connect and change, DAC reference −3 dB while the sink takes absolute volume), address text in written order. User check 17 |
| BRAND-1 | done (host) | `docs/brand/palette.toml` (dark/light = firmware Dark/Light with Blue; `link`, `accent` web-only; WCAG AA checked for the web and after RGB565), `cargo xtask brand [--check]` → `docs/brand/generated/` (mono/small lyre, wordmark in the firmware's bold font, favicons + ICO, apple-touch, manifest icons, 1200×630 social card, `palette.css`). Firmware theme tables unchanged; generating them, the boot logo and firmware icons are follow-ups. User: approve palette and logo variants |
| DOC-2 | done (host) | README rewrite, `docs/user/` (manual index, install/update/restore, keys, Music, Now Playing, Files, Pictures, Bluetooth, Settings, Performance, troubleshooting, FAQ; each ≤ 600 words, checked by `site --check`), screenshots `docs/shots/` from goldens (`xtask site shots`). User: read the manual once |
| DOC-3 | done (host), except the clean-container CI run | `docs/dev/getting-started.md`, `architecture.md` (SVG diagrams in `docs/dev/img/`), `how-to/` (8 guides). `#![warn(missing_docs)]` on `orpheus-codecs`, the wav/mp3/flac/vorbis/speex codecs, `orpheus-fs`, `orpheus-settings` |
| DOC-4 | done (host); workflows not run (no CI here) | CONTRIBUTING, CODE_OF_CONDUCT (Contributor Covenant 2.1), SECURITY, CHANGELOG, issue/PR templates, workflows `gate.yml`, `docs.yml`, `pages.yml` (actions pinned by SHA) |
| WEB-1 | done (host); not published | `cargo xtask site [--check]` → `target/orpheus/site/`; Home, Manual, Developers, Hardware, Changelog, FAQ, Try it and Install (placeholders for WEB-2/WEB-3). Deploy only with repository variable `PAGES_ENABLED=true`. User: review the site |

## 3. Session log

### 2026-11-24 — FM-1 fix after the first device run: radio RAM overlapped the step counter's, id read fallback, `fm on` stops music (worker, no hardware)

From the lead's run of `97e60988` (earphones in, music stopped): `fm on`
gave `phase=playing khz=0 chip=0000`, shadow 02h `0x4000` (ENABLE clear),
04h `0x0000`, `groups=2097152`, RSSI 0, `fm scan` 0 stations; with music
playing `fm on` ended `phase=off` and the music went on.

* **Cause: two SRAM3 blocks on top of each other.** The merge placed the
  step counter's state at a fixed `0x307A00` (1,144 B, to `0x307E78`) and
  the radio's at a fixed `0x307C00` (848 B); `init_chain` initialised the
  sensor after the radio, and the sensor task kept writing its state over
  the radio: `khz=0`, the garbage RDS counter, a shadow that was never the
  loaded table (04h would hold DE = 50 µs after any load), chip 0. The
  register sequence itself ends with ENABLE set (now a test). The three
  blocks did not fit the 2,044 B window together (252 + 1,144 + 848).
* **Fix**: the window is chained, each address computed from the end of
  the previous block and asserted: battery logger `0x307800` (252 B),
  step counter `0x307900` (1,144 B), radio `0x307D78` (radio 360 B + the
  UI's radio text 155 B = 520 B, to `0x307F80`), card I/O counter
  `0x307FFC`. The presets (332 B) moved back to SRAM0 `.bss`.
  `init_chain`'s duplicated `drain::init()` (merge) removed.
* **Chip id**: the driver never read at 0x11; it reads the vendor's way
  (bytes 8–9 = register 0Eh of a sequential read at 0x10 after the soft
  reset). Now, if those bytes are not 0x5804/0x5801, register 00h is read
  by random access at 0x11 (as the bring-up image did) and used if it is a
  0x58xx id. `Port::read_reg` (write of one index byte to 0x11, repeated
  start, read).
* **`fm on|tune` with music playing**: the task now asks the card task to
  stop the music (once) while a start waits on the DAC, then starts.
* Tests: `orpheus-fm` 42 (+4: every write after the table keeps ENABLE and
  DHIZ and the playing 02h has DHIZ/DMUTE/RDS/ENABLE; identify uses 0x10
  first; falls back to 00h at 0x11; a radio start with no id in 0Eh loads
  table A). Gate: see the commit.
* **RAM**: `__stack_bottom` `0x132678` (main `bd32f17d`) → `0x1327C0`
  (+328 B, the presets). Against the base before FM-1 the radio now costs
  about +784 B of SRAM0, and 520 B of SRAM3.
* Lead's re-run: `fm status` after boot (`phase=off`), `fm on` → expect
  `chip=5804`, shadow 02h `0xC459` or so (DHIZ, DMUTE, RDS, ENABLE + 24 MHz
  bits `0x0450`), 04h `0x0C00`; `fm tune <local>` → RSSI > 0; `fm scan`;
  `playdir …` then `fm on` → music stops, radio plays; `acc read` still
  works (step counter's state moved); end with `fm off`.

### 2026-11-24 — M12: BRAND-1, DOC-2, DOC-3, DOC-4, WEB-1 (worker, no hardware)

* **Brand** (`docs/brand/`): one palette with roles; the dark and light
  schemes are exactly the firmware's Dark/Light themes with the Blue accent
  (RGB565-equal; a unit test and `xtask brand` report drift). Added web-only
  `link` (#6ea8ff dark, the lyre blue is 3.9:1 on the dark background) and
  `accent` (amber). All pairings ≥ 4.5:1 (text) or ≥ 3:1 (non-text), web
  and RGB565. `xtask brand` renders with `resvg` 0.48 (no default features:
  no system fonts; all text in the generated SVGs is the firmware's own
  bitmap font as rectangles, so output is deterministic). `brand --check` is
  a gate step. Generated: 12 files, 44 KB.
* **Docs**: README (logo, three goldens ×2, status, Try it/Install with
  "coming" browser paths, Build in four commands); `docs/user/` 12 pages,
  checked against this log (e.g. Lyrics/Levels views, rename/delete,
  playlists, speed, rotation, other languages and CJK fonts on the player
  marked **in progress**); `docs/dev/` getting started, architecture with
  five SVG diagrams (looked at in Chromium, light and dark), eight how-tos;
  CONTRIBUTING, Contributor Covenant 2.1 (contact: the repository's private
  advisory form), SECURITY (reporting; the authenticator's at-rest limits of
  06 §5.2), CHANGELOG (Unreleased from this log).
* **Site** (`xtask site`, `pulldown-cmark` 0.13): templates in `site/`,
  Markdown links rewritten to pages (other repository files → GitHub),
  GitHub-style heading ids, screenshots ×2 nearest-neighbour from
  `sim/golden` (`docs/shots/` for the README/GitHub, 20 files, 120 KB),
  light/dark/auto theme (system preference without JavaScript), skip link,
  responsive tables, no external requests except links. `site --check`:
  internal links and fragments of the built site, repository links exist
  and are not under `docs/hw`/`docs/vendor`/`local`, every referenced
  screenshot has a golden and `docs/shots/` is current, manual pages
  ≤ 600 words, Home ≤ 500 KB (measured: 68 KB with everything it loads).
  71 files, 612 KB. `test golden --bless` also refreshes `docs/shots/`.
  Looked at in headless Chromium: Home (light, 1280 px), Keys (dark,
  390 px), Architecture (light and dark).
* **CI** (`.github/workflows/`, not run here; YAML parsed): `gate.yml` host
  gate (fmt, clippy, tests incl. goldens, brand and site checks) and an
  optional firmware job (`continue-on-error`); `docs.yml` markdownlint
  (`.markdownlint-cli2.yaml`, reader-facing docs only) and lychee (internal
  on push/PR, external weekly); `pages.yml` builds on every push/PR (PR
  artefact preview) and deploys only when the repository variable
  `PAGES_ENABLED` is `true`. Actions pinned to commit SHAs (checkout v7.0.1,
  upload-artifact v7.0.2, upload-pages-artifact v5.0.0, deploy-pages
  v5.0.1, markdownlint-cli2-action v25.1.0, lychee-action v2.9.0).
  actionlint and markdownlint were not run locally.
* **Found while writing (not fixed here, firmware unchanged)**: the
  firmware's `dual_boot()` returns `true` in the full layout, so the power
  menu and Settings › System offer "Start original firmware" although the
  stock firmware is erased (documented as a known issue); the language
  setting offers Russian/Japanese but the firmware loads no language packs
  and has no fallback font; `xtask dev` help still says `stage` writes to
  `0x762000`; `backup-verify` accepts only the reference player's backup
  hash and the device tools need `ORPHEUS_DIR` (companion repository) for
  ymon/nordump payloads, so the command-line install is reference-player
  only (stated in the manual); `install-boot` (07 §2) does not exist yet.
* **Follow-ups**: firmware theme tables, boot logo and 16/24/32 px icons
  generated from `docs/brand/`; a web-font subset of the bitmap font for
  headings; rustdoc on the site and `missing_docs` for the other crates
  (counts with `-W missing-docs`, 2026-11-24: atj2157-pac 831, orpheus-apps
  384, orpheus-ui 238, orpheus-proto 197, sim 157, opus 122, orpheus-dsp
  116, orpheus-library 109, orpheus-bt 106, orpheus-audio 98,
  orpheus-player 92, orpheus-usb 80, orpheus-img 77, orpheus-power,
  orpheus-host, orpheus-meta, orpheus-otp and conformance 61 each,
  orpheus-sdcard 43, orpheus-index 30, orpheus-pager 28, ogg 27,
  orpheus-input 26); the clean-container getting-started CI job, HTML
  validation and Lighthouse CI (09 §8); translations of the manual; the
  battery-life table on Home after user check 19; WEB-2 and WEB-3.
* Gate (host): see the commit messages. Not run: device steps (no
  hardware in this package), CI workflows.
| FM-1 | **first device run 2026-11-24: radio RAM overwritten by the step counter's (fixed, not yet re-run)** | `orpheus-fm` (host-tested: RDA5807 register model, the vendor's init tables VERBATIM, tune, hardware seek with region limits / wrap / Wide's two bands / the 96 MHz harmonic, auto-scan, RDS PS/RT, volume pairs, presets) and the firmware `fm` task over the software I2C (writes only to `0x10`), FM bypass into the amplifier (`dac::fm_*`, VERBATIM, without the vendor's amplifier power cycle), tuner in standby whenever the radio is off. `fm on/off/tune/seek/scan/status/route` developer commands. User check 20 |
| UI-5a | **built (host); on the player with FM-1** | Radio app (frequency, band, signal bars, ST, preset, RDS name and text, Prev/Next step and hold-seek, Up/Down presets, OK mute / hold stop, Menu: Save/Remove preset, Presets…, Auto-scan with its question, Force mono, Region, Stop), Presets list (move, delete), antenna note, FM source in the status bar and mini-player; on Home. Not done: preset rename, Record, Output (05 §7.3) |

## 3. Session log

### 2026-11-24 — FM-1 + UI-5a: tuner driver, FM bypass, radio service, Radio app (worker, no hardware)

* **`orpheus-fm`** (new host crate, `no_std`, no `unsafe`, 38 tests with a
  register-level fake tuner, `fake` feature for the simulator):
  * `chip`: RDA5807 sequential access at `0x10` (writes from 02h, reads
    from 0Ah), the vendor's three init tables VERBATIM (id 0x5804 → table
    A, 104 bytes), our fields patched in: start muted, RDS on for the
    0x5804 class, region band/spacing, de-emphasis 50 µs outside the
    United States (the vendor leaves 75 µs), soft mute, seek threshold.
    Only 02h–05h for volume (no stray ninth byte).
  * `region`: Europe 87.5–108 / 100 kHz, United States 87.9–107.9 /
    200 kHz (75 µs), World 87.5–108 / 50 kHz, Japan 76–95, Wide 65–108
    (two hardware bands; 65 not 64: 07h bit 9 is set in every vendor
    table).
  * `radio`: the service as a time-driven state machine. Start in the
    vendor's order: soft reset, 10 ms, identify + table, 500 ms
    (VERBATIM), gain 0, bypass pre-charge/connect/release with 100 ms
    waits, tune, volume, unmute, gain ramp 1 step / 5 ms. Stop: mute, ramp
    to 0, bypass off, gain back to 7 (music), tuner standby. Hardware seek
    stopping at the band edge; the region's limits, wrap, Wide's band
    crossing and the 95.9–96.1 MHz harmonic in software. Auto-scan by
    hardware seeks, 30 strongest, strongest first. Writes of 02h wait
    while a hardware seek runs (they would stop it). 5 failed status
    reads end the radio.
  * `rds`: PS (each pair confirmed twice), RT 2A/2B (contiguous, A/B flag
    clears), block-B error gate, duplicate groups once, RDS charset → UTF-8
    (upper half from memory of Annex E, **[inferred]**).
  * `presets`: 30 × (frequency, RDS name) in their stored form (332 B,
    settings-store key `0x0710`).
  * `volume`: the vendor's 32 (tuner volume, amplifier gain) pairs on the
    UI's 0–30 scale.
* **Firmware** (`fw/firmware/src/fm.rs`, main executor): the radio, the
  presets and the UI's radio text in SRAM3 at `0x307C00` (848 B, between
  the battery logger's state and the card I/O counter), shared by the
  `fm` task, the UI and the USB task through synchronous closures only.
  At boot one 2-byte write puts a tuner left running into standby.
  Music and radio exclusive: the UI stops the music first; the task holds
  a starting radio until the DAC stopped (≤ 2 s) and stops the radio when
  music starts. `atj2157_hal::dac::fm_precharge/connect/release/
  disconnect` (VERBATIM steps of `AUD_DEV` API 15/16 type 1) **without**
  the vendor's amplifier power cycle and DIGCTL[3:2] write (the DAC path
  is unchanged; first suspects if FM is silent, U-FM-3). Earphone sense
  (key ladder 1 idle level, 11 samples) → antenna note. Volume keys set
  the radio's pairs. The last frequency is saved after 5 s of playing.
  Auto-off counts the radio as playing; power-off (and low battery) stops
  the radio first (tuner standby, ≤ 300 ms).
* **Developer commands** (`cargo xtask dev cmd "fm …"`): `fm on`,
  `fm off`, `fm tune 101900`, `fm seek up|down`, `fm scan`, `fm status`,
  `fm route on|off` (the bypass alone, for U-FM-3). Replies wait for the
  operation (≤ 25 s): phase, kHz, RSSI, stereo, mute, volume, gain, chip
  id, RDS PS/RT/PI/groups/bad, errors, registers 02h–07h (shadow) and
  0Ah–0Fh (last read), `PA_ANTIPOP`, `PA_CTL`; `fm scan` lists kHz/RSSI.
* **Radio app (UI-5a)**: `RadioService` in `orpheus-apps`, screens
  `Radio` and `RadioPresets`, strings (en; ru/ja test packs), Home opens
  it. Simulator back end: the same `orpheus_fm::Radio` over the fake
  tuner (`sim/src/radio.rs`; `set:radio=KHZ|off`, `set:presets=demo`,
  `set:antenna=0`, `set:tuner=absent`, `set:seekreads=N`).
* **Tests**: `orpheus-fm` 38; `sim/tests/radio.rs` 13 (music/radio
  exclusive, start where it was, mute/stop and the ramp order, steps and
  wrap per region with the register read-back, seek, presets, list
  reorder/delete, auto-scan question and result, region/mono, RDS,
  volume pairs, antenna note, missing tuner). Goldens: 110 new (18
  journeys × dark/light × latin/cyrillic/cjk + 2 amber; looked at),
  6 changed (`app_not_yet_*` now opens the Recorder: Radio is an app).
  Unifont subset +9 glyphs for the new Japanese strings.
* **RAM**: `__stack_bottom` 0x132238 → 0x132400 (**+456 B**): resident
  83,316 → 83,604 B (+288: text +176, mostly `__aeabi_memset8`, newly
  linked by the radio's 8-byte-aligned state; merged constants and
  strings in `.rodata` +112), `.bss` +168 B (`fm` task pool 112 B, its
  signal and flags, `Fw` fields). SRAM3: 848 B at `0x307C00` (radio
  360 B, presets 332 B, the UI's radio text 155 B). Paged: 78 code pages
  (was 75). Gate: host fmt, clippy, `cargo test --workspace`, `xtask
  test golden`; fw fmt, clippy; `xtask build`, `check-resident` pass.
  `size --check` not run (worktree path length).
* **Not run on the player.** Open: U-FM-2/3, whether the 0x5804 part
  decodes RDS, the earphone window (U-AUD-5), TWI1 (U-FM-1 rest), the
  shared bus with the accelerometer (SENS-1 adds a lock; nothing else
  uses the bus yet). User check 20.
### 2026-11-24 — SENS-1, UI-5c, UI-5d: step counter, Steps app, Stopwatch · Timer · Alarm (worker, no hardware)

* **`orpheus-steps`** (new crate, `no_std`, host-tested): the step
  detector (integer only: magnitude, gravity removed over ≈ 1.3 s,
  ≈ 4 Hz low-pass, decaying envelopes with ± ¼-swing hysteresis, minimum
  swing by Sensitivity 0.16/0.10/0.06 g, step interval 270 ms – 2 s, four
  regular candidates before counting, a "too fast" rule on the unfiltered
  signal against shaking); `History` (today + six days, day rollover,
  clock set back, merge with steps counted before the card was read) and
  `/.orpheus/steps.bin` (`OSTP`, version 1, CRC-32, 68 B); the trace CSV
  format; distance and kcal (05 §7.7). Tests: synthetic traces at 25 and
  50 Hz (walk, slow, brisk, run, pocket, clipped sprint within ± 5 %, eight
  seeds each; **0 steps** on a table for 10 min, handled, on a bus, shaken
  at 3.8–8 Hz; walks with stops; sensitivity order; the user check's
  200 steps twice) and `traces/*.csv` with `# truth=N` (three synthetic
  files committed; captures from `acc trace` go there). Shaking slower
  than ≈ 3.7 Hz is a running cadence and counts (documented).
* **Firmware (SENS-1)**: `yoton_bsp::accel` (id check, the vendor's open
  sequence steps 1–4, 7–9, 14 marked VERBATIM: ± 2 g, ODR code 6 = 62.5 Hz;
  suspend at boot and when nothing counts; one 6-byte burst read). **No
  FIFO, no interrupt**: the part is not identified beyond id `0x13`
  (U-ACC-1; `acc regs` dumps `0x00–0x3F` to tell). `firmware::sensor`
  task on the main executor: 25 Hz while Count in background is "While on"
  (default), the Steps app is open or a trace runs; the detector, the RTC
  day check and the history once a second over the batch; per-sample code
  in the new `hot.toml` cluster `steps-sample` (4 lines at `0x222000`,
  after `play-block`, which does not move). State in SRAM3 at `0x307A00`
  (history, detector, 64-sample trace ring). The card task reads
  `steps.bin` at mount, writes it 10 min after the first unsaved step, at
  a day change, at power-off and before a USB hand-over (never while the
  clock is invalid), trace rows when 32 wait; while playing only with
  `ROOMY` headroom (ledger cause `serve`), like the battery log.
* **Developer commands**: `acc read` (one sample, magnitude in mg, chip
  id, ODR, mode, rate, counting, samples, I2C errors; wakes a suspended
  part for the read), `acc regs`, `acc trace <secs>` (≤ 3600) /
  `acc trace stop` (reply: file, written, lost), `steps status` (today,
  the week, present, counting, background, app, dated, file loaded,
  detected, samples, errors, saves), `alarm status` (RTC alarm registers,
  now, wake-from-off flag).
* **UI-5c Steps** (05 §7.7 as built): tabs Today · Week, goal bar,
  distance/kcal, seven bars with the goal line, Up/Down day; Menu: Daily
  goal, Stride, Weight, Units, Count in background (Off / While on),
  Sensitivity, Reset today. Settings `0x0D20–0x0D25`. Strip hints: "No
  clock: no history", "Counts only here". `Tabs` widget in `orpheus-ui`
  (`draw_tabs`: side by side, or the active name between ◂ ▸).
* **UI-5d Stopwatch · Timer · Alarm** (05 §7.8 as built): state in the
  service (`orpheus_apps::tools::Tools` in `Fw`), so they run in the
  background; stopwatch timed from the key-down edge, 20 laps; timer with
  the last five durations as presets, h:m:s picker (the date/time picker
  gained timer and alarm modes); four alarms (time, days or once, on),
  snooze 9 min, a one-time alarm switches itself off; the alert screen
  over any screen lights the display, flashes the backlight and plays a
  2 kHz beep pattern through the headphones (`audio::alert_tone`, in the
  card task's idle loop or while paused: the music pauses for it and
  resumes); status bar bell (alarm on, timer running, snoozed) and dot
  (stopwatch running); auto-off waits while the stopwatch or timer runs.
  The RTC alarm is armed with the next alarm (`rtc_clock::set_alarm`,
  `alarm_state` added); ringing while on comes from the once-a-second
  minute check. **Wake from off: not done** — `services::ALARM_WAKE_FROM_OFF
  = false` (PWR-3c: U-PMU-3's alarm case and a wake reason at start are
  missing; 03 §11); the Alarm tab says "Rings only while on".
* **Simulator**: fake accelerometer (`sim/src/steps.rs`: the real detector
  at 25 Hz on a synthetic walk or noise; `set:walk=`, `set:steps=`,
  `set:week=a/b/…`, `set:sensor=off`), fake clock (`set:clockrun=1`,
  `set:clock=HH:MM:SS`, `set:weekday=`, `set:clockvalid=0`),
  `set:alarmwake=1`. 171 new goldens (`steps_*`, `tools_*`: dark/light ×
  latin/cyrillic/cjk, amber), looked at; none of the 620 existing changed;
  791 in all. Behaviour tests: `sim/tests/steps.rs` (8),
  `sim/tests/tools.rs` (12: key-edge timing exact over 60 s, laps, reset
  confirmation, background with indicators, timer alert once and the
  screen lit from off, ringing ends after a minute, presets, alarm days,
  one-time, snooze, no alarm without a clock, alarm edits).
* **RAM**: `__stack_bottom` 0x132238 → 0x132498 (**+608 B** SRAM0: `Fw`
  +304 (`Tools` ≈ 220, 23 more settings), sensor task pool 120, its signal
  12, the rest task futures). The sensor state is in SRAM3. Async frames:
  sensor task poll 68 B, `steps_save`/`trace_write` 980 B each (as
  `drain_flush_rows`, 1,004 B, on the same path), audio task poll 4,992 B
  (unchanged). Resident 83,316 → 83,484 B (+168: the panic-path strings of
  `sensor.rs` and `accel.rs`; `clocks.rs` and `detector.rs` made
  panic-free); code pages 75 → 79, slot 835,584 → 868,352 B.
  `size --check` not run (fails in worktrees, path length).
* **Not done**: Month tab, "Always" counting, alarm actions other than the
  beep and the volume ramp, stopwatch across a restart, wake from off
  (PWR-3c), the alert through Bluetooth (it plays at the DAC).
* **Not run on the player.** Lead's test plan: in the worker's final
  report (dev commands first, then user checks 20 and 21).

### 2026-11-24 — PWR-5 rework and AU-5 fixes: playdir reboot, browsing while playing, remount, playdir during playback, bit rate (worker, no hardware)

From the lead's run of `f6492092` (merged locally, not kept): `playdir`
rebooted within ≈ 5 s (watchdog, no fault record), also with the logger
off and the governor off; the logger never wrote while idle.

* **The reboot: code layout, not logger code.** With the logger off no
  new code runs on the playback path (an atomic load per block, a counter
  per card operation). What changed: the paged layout (74 → 75 pages;
  `AnyDecoder::decode` moved 9 KiB, `play_file` 3.6 KiB), and SRAM0
  (+424 B). The stack has an MPU guard (an overflow leaves a StackGuard
  record; the lead measured 5,972 B free during playback), so the paging:
  the lead measured ≈ 2,700 line loads/s and 18 loads per decode call on
  `2e9bf3a4`: MP3 already runs close to thrashing. Found on the way:
  `AnyDecoder::decode` holds the MP3 frame code inlined (≈ 7 KiB, paged,
  about 8 lines per frame) although `orpheus_codec_mp3::*` is resident.
* **New `hot.toml` cluster `play-block`** (after the Opus clusters,
  67 lines at `0x211000`): `AnyDecoder::decode`, the card task's poll,
  the card read path of a refill (fill_in, fs read, cluster_at, fat_get,
  cache load, card and SD read), the per-block small functions (task
  poll, PCM push/headroom, note, player update, `Server::feed`,
  `Player::handle`), then `play_file`. Its place no longer moves with
  unrelated code. The order of the groups was chosen offline: hot lines =
  the small functions whole, and in the large ones the entry and the call
  sites into the per-block targets; about 41 hot lines for 32 physical
  ones; lines shared between different pages: 8 in this order (11 with the
  small functions first), against ≈ 27 on `2e9bf3a4` and 30 on `33b9db6e`
  unpacked. An estimate only: **line loads/s with MP3 and Opus are the
  measure** (the Opus clusters' places are unchanged; its per-block
  firmware code moved).
* **SRAM0 back to about zero** (stack bottom +88 B against `2e9bf3a4`:
  `Server` +104 B for the listing job, `PlayerSnap` +8, `Fw` +16, USB pool
  −40): the logger's state and the shared flags (Performance, backlight,
  screen, developer rows, Keep awake) live in SRAM3 at `0x307800`
  (`drain::Shared`, initialised in `audio::init_chain`), the card I/O
  counter in the last word of SRAM3; no panic sites in the logger (each
  kept a file path in the resident `.rodata.str1.1`); 4/8/16-byte strings
  avoided (merged into resident `.rodata.cst*`); the UI's developer
  handlers not inlined (the screen dispatch frame grew by 176 B); the
  card task's drain functions in `audio.rs`. Resident text, rodata and
  task pools as on `2e9bf3a4`.
* **"Never wrote"**: by design rows went out only every 5 rows (5 min);
  the header at once. Now the first row is written after the first
  minute, then every 5.
* **Browsing Files while playing** (8–14 underruns per track, all
  `serve`): a listing is made in pieces (`media::Job`, `Server::step`),
  one per chore between ring refills, each ≤ 8 ms (`PIECE_US`) and
  ≤ 32 directory entries or 8 rows: the counting pass, the index check,
  the focused row (now within 256 rows while playing, was 1,024: 8 KiB of
  index per piece), the rows of the window, the letter table. The window
  is published as it fills with `complete: false` (the UI asks for
  nothing meanwhile). Scrolling (`Want`) with a sorted listing goes the
  same way; while paused the pieces run back to back; a job left over
  when playback ends finishes in the idle loop. Idle browsing unchanged.
* **Remount after `usb msc ro` / `usb dev`**: the USB task resets the SD
  controller and re-initialises the card (`Card::reinit`) before handing
  it back (a host may stop mid-transfer). In the card task it added the
  card's size (bounce buffer) to the task's pool (+616 B); in the USB
  task the pool did not grow.
* **`playdir` while a track plays**: the developer channel stops the
  playing track (`request_stop`) before queuing the command (the card
  task reads commands only between tracks), so the new queue starts.
* **Bit rate 0 kbit/s**: the firmware always passed 0. Now
  `orpheus_apps::services::average_kbps`: the decoder's average, else the
  file's size over its duration (host-tested).
* Tests: `orpheus-power` 23, `xtask drain` 7, `average_kbps`; goldens
  for the Developer rows re-blessed ("File" instead of "Log file", a
  shared string). Gate: see the commit. **Not run on the player.**
* Lead's plan: (1) `playdir` a 48 kHz MP3 and an Opus album: `stats`
  line loads/s and `status` `dec:` loads per call against `2e9bf3a4`,
  no reboot; (2) Files while playing a 48 kHz MP3, scroll a large folder:
  `ub:` serve 0; (3) `usb msc ro`, mount on the host, `usb dev` while
  mounted: `status` shows the card mounted; (4) `playdir A`, then
  `playdir B` while A plays: B starts; (5) Now Playing › Info bit rate;
  (6) `drain start smoke stay`: `written=1` after ≈ 1 min.

### 2026-11-24 — PWR-5 (D-4): Performance setting, battery logger, drain report (worker, no hardware)

* **Performance setting**: Settings › System › Performance, Normal /
  Maximum, store key `0x0C07` (default Normal; stored settings 64 → 65,
  the boot-time key buffer is now sized from `STORED`). Strings en/ru/ja.
* **Governor decision as a pure function**: `orpheus_power::governor`
  (`Governor::step(Input { idle_pct, current, floor, perf })`), used by the
  firmware's 1 Hz governor. Normal = today's behaviour (Base ↔ Mid ↔ High,
  up below 25 % idle, down after 3 s above 70 %, floor respected), with one
  change: a `Max` level found under Normal goes to High at the next step
  (so `clock 288` with `gov on` no longer sticks; pin with `gov off` as
  before). Maximum: the step above High (or Turbo) is Max; down Max → High
  with the same hysteresis. Tests include "never Max when Normal"
  (2,000 random loads × every start level × every floor), hysteresis and
  floor.
* **Battery logger** (`orpheus_power::drain`, host-tested; firmware
  `drain.rs`): the power task (already 1 Hz) samples while it runs; a row
  per minute: `uptime_s,mv,mv_raw,percent,charge,cable,clock_mhz,s_low,
  s_base,s_mid,s_high,s_turbo,s_max,playing_s,backlight,screen,bt,card_io`
  (seconds per clock level and playing seconds since the last row; card
  I/O = block-device operations, new counter `yoton_bsp::card::IO`).
  Header line with label, file number, perf and stay; `# lost N rows`,
  `# stop`, `# power-off` comments. 6 rows in RAM, a write is due at 5.
  Writes go through the card task's file API: idle at once
  (`media::Req::Drain` wakes the idle loop), while paused (DAC stopped),
  while playing only after a refill with `ROOMY` headroom — never forced
  like the other chores (ledger cause `serve`); the file number (scan of
  `/.orpheus/test/`) only in the idle loop or a served request. Flushed at
  `drain stop`, at power-off (auto-off and low battery included: the UI
  ends the log before the card task's shutdown flush) and before a card
  hand-over to USB. It adds no timer and does not touch auto-off unless
  started with `stay`. In test mode power-off reboots before the flush
  (unchanged rule 4 path), so the power-off row is only seen unplugged.
* **Turned on by** `cargo xtask dev cmd "drain start [LABEL] [stay]"`,
  `"drain stop"`, `"drain status"` (reply: state, file, written, pending,
  lost, stay), or Settings › System › Developer (hidden: five OK presses in
  About show it until restart): Battery log (label `normal`/`max` from the
  Performance setting), Keep awake, Log file, Rows written.
* **`cargo xtask drain report FILE…`**: per file duration, mV first → last
  and drop, mV/h from the end points and from a least-squares fit (with
  r²), share of time per clock level and playing, screen-on rows, card I/O
  per minute; rows with the cable in are left out of the voltage figures;
  a comparison table (and the second file's fit vs the first's). Tested on
  synthetic CSVs and on output of the firmware's own formatter.
* Goldens: 24 new (System scrolled to Performance, the choice, the About
  unlock toast, the Developer rows; dark/light × latin/cyrillic/cjk; looked
  at), 8 changed by one row more in System (scroll bar). 620 images.
* **RAM** (this first version; see the rework entry above): `__stack_bottom`
  +424 B (`drain::LOG` 244 B, flags, resident panic-path strings and
  constants). Reworked to about zero.
* Docs: 03 §2.1/§2.4 (as built), 08 PWR-5 row, `docs/user/performance.md`,
  user check 19.
* **Not run on the player.** Lead's test plan: in the final worker report
  of this session (drain start/status/stop, a 6-minute playback run with
  `status` underrun causes, Performance = Maximum reaching level 5 in
  `stats` under load and never at Normal, auto-off with and without
  `stay`), then user check 19.
* Open: whether a card write at `ROOMY` headroom (≈ 52 ms) ever underruns
  (the `ub:` serve counter will say; one write per 5 minutes); `card_io`
  counts operations, not blocks.

### 2026-11-24 — AU-8 (Opus) on the player: 16 KiB ring and paging fixed (worker, no hardware)

From the lead's runs: `t_opus.opus` (one-second pages) ended with "stream
needs a larger ring", `t_opus_p.opus` with "decoder too slow for this
player", also at 288 MHz. Both are fixed on the host side; the device run
is the lead's (test plan below).

* **Ring.** ffmpeg and opusenc write one-second Ogg pages: measured over
  the Opus corpus 1.0–2.5 KB (SILK), 3–8 KB (≤ 64 kbit/s), 10–28 KB
  (96–256 kbit/s); Vorbis q6–q10 17–47 KB. The readers asked the ring to
  hold any page up to `MAX_PEEK` (28 KiB) whole before using it; the
  firmware ring is 16 KiB. New `orpheus_ogg::CHECKED_PAGE` = 8 KiB (half
  the ring, its refill threshold): smaller pages are CRC-checked before
  use as before, larger ones are read packet by packet with the CRC
  checked as they stream past (a mismatch is reported after their
  packets, as it already was above 28 KiB). Page `NeedData` ≤ 8 KiB. The
  Vorbis reader uses the same bound (Vorbis files with large pages failed
  the same way). Conformance now also decodes every Ogg Opus/Speex/Vorbis
  file through a 16 KiB ring ("firmware ring": identical to the linear
  decode).
* **How the speed problem was measured** (no player): a bare-metal
  harness of the Opus decoder built with the firmware's profile
  (thumbv7em, opt-level s, fat LTO; function sizes identical to the
  firmware's) runs in an emulator (unicorn, Cortex-M4) and records every
  executed block and rodata read per decoded frame; the trace is mapped by
  symbol onto the firmware's `layout.json` and replayed through
  `orpheus_pager::sim::Machine` with the real `pages.bin`. Tools in the
  worker's scratch directory (not committed: Python + unicorn); the
  method is reproducible from this description.
* **Measured before** (decoder alone, steady state, per 20 ms frame,
  `music_96k`, CELT stereo): 799 k instructions, 44–46 code lines and
  21–23 rodata lines touched (the caches hold 32 and 16), **61.5 NOR line
  loads (max 161) and 258 faults per frame** ≈ 11 ms of line reads per
  20 ms — with ≈ 7 ms of compute at 144 MHz it cannot keep up at any
  clock, as seen. Causes: the Opus frame code was spread over 9 pages
  (direct-mapped slot conflicts: `celt::decoder::decode` and
  `mdct_backward` on the same physical lines), `OpusDecoder::decode` was
  inlined into the 26 KB `AnyDecoder::decode`, the range decoder was
  inlined at every call site, the hot tables were scattered (anonymous
  constants among them), and a one-second page made the 1 KiB CRC table
  and `PVQ_DATA` (5 KB) part of every frame. SILK WB mono: 304 k
  instructions, 37–39 code lines, 32 loads per frame.
* **Changes** (decoder output bit-identical: PCM hashes of all 43 corpus
  files whole and through a 16 KiB ring, the RFC 8251 vectors):
  * `hot.toml` clusters `opus-celt` (25 lines), `opus-frame` (7 lines:
    packet loop, Ogg packet reader, range decoder, output) and `opus-silk`
    (23 lines), after `gapless-join`; a CELT stream runs in
    `opus-celt` + `opus-frame` (32 contiguous lines: no physical-line
    conflict with itself), a SILK stream in `opus-frame` + `opus-silk`.
    The CELT rodata (MDCT and FFT twiddles of the sizes in use, bit
    reversal, window, allocation and energy tables, 15.5 KB) fills rodata
    group 0; the SILK tables group 1.
  * Code compaction: `OpusDecoder::decode` not inlined into the codec
    dispatch; the range decoder's primitives out of line (decode 16.1 →
    9.1 KB, SILK `decode_pulses` 5.5 → 1.5 KB, crate 117.6 → ~105 KB);
    CELT concealment, post-loss energy and prefilter fold `#[cold]` out of
    `decode`; `opus_dec::decode_frame` keeps only the CELT path inline
    (SILK/hybrid, redundancy, transitions, long SILK frames in their own
    functions); the soft clipper's slow path out of line.
  * Rodata: hot `const` tables made named `static`s (clusterable): CELT
    locals, all SILK tables; MDCT twiddles split per size
    (`MDCT_TRIG_1920/960/480/240`, selected through `FftState`, no jump
    table); the PVQ codebook row computed per band (libopus's
    `SMALL_FOOTPRINT` `ncwrs_urow`/`cwrsi`/`uprev`; the table version
    stays for out-of-range input; a test compares both over every
    defined (N, K)) instead of reading the 5 KB U(N, K) table; the Ogg CRC
    by nibbles (64-byte table instead of 1 KiB, all Ogg codecs).
  * Compute: `sqrt` estimate-and-correct (same correctly rounded result,
    checked for all 2^31 positive finite floats; ~⅓ of the instructions),
    soft clipper skipped when nothing exceeds ±1 and no clipping is
    carried over (identical output), float→i16 in place without a copy
    and through `i32` (one `vcvt`). The PVQ row computation costs
    ≈ +55 k instructions per stereo frame; net 799 k → 760 k.
  * The PVQ row lives in the CELT scratch (arena), not on the stack:
    Opus arena high-water 59,488 → **60,008 B** (stereo; mono +520 B),
    firmware assert updated (65,536 − 2,048 spare − library state still
    fits).
* **Measured after** (same method, the firmware layout of this commit,
  decoder alone, 10 warm-up frames skipped, per 20 ms block unless noted):

  | File | Instructions | Code / rodata lines | NOR loads | Faults |
  |---|---|---|---|---|
  | music_96k (CELT stereo) | 760 k | 32 / 12 | 0 | 7 |
  | celt_cbr_128k | 889 k | 32 / 12 | 0 | 7 |
  | music_256k | 1,175 k | 32 / 12 | 0 | 7 |
  | impulse_96k (transients) | 992 k | 32 / 9 | 0 | 3.5 |
  | opusenc_96k | 760 k | 32 / 12 | 0 | 7 |
  | mono_48k | 434 k | 32 / 12 | 0 | 7 |
  | celt_fd10_96k (10 ms frames) | 434 k / 10 ms | 32 / 12 | 0 | 7 |
  | celt_fd2.5_96k | 165 k / 2.5 ms | 32 / 9 | 0 | 7 |
  | silk_wb_16k_mono | 265 k | 29 / 7 | 0 | 8 |
  | silk_nb_8k_mono | 179 k | 29 / 8 | 2 | 12.5 |
  | silk_wb_20k_stereo | 430 k | 29 / 5 | 0 | 8 |
  | hybrid_fb_32k | 690 k | 32–54 / 12–21 | 17 (max 61) | 24 |
  | hybrid_swb_24k | 690 k | 53 / 17 | 43 | 50 |
  | opusenc_speech_24k_60ms (hybrid) | 744 k | 52 / 17 | 52 | 58 |

  Hybrid (SILK + CELT in one frame, ≈ 24–32 kbit/s) needs both clusters,
  54 lines: it still reloads ~20 lines twice per frame.
* **Expected on the player** (estimate: cycles ≈ 1.3 × instructions, the
  FPU benchmark scales linearly with the clock; NOR line 180 µs; plus the
  playback loop's own paged lines, not measurable in the emulator, guessed
  at 4–10 loads per frame): CELT 96 kbit/s stereo ≈ 6.8 ms compute + ≤ 2
  ms paging per 20 ms at 144 MHz (≈ 45 %), ≈ 3.4 ms + ≤ 2 ms at 288 MHz;
  256 kbit/s ≈ 10.5 ms at 144 MHz (≈ 60 %). SILK WB mono ≈ 7 ms per 20 ms
  at 48 MHz, 2.4 ms at 144 MHz; SILK stereo 20 kbit/s ≈ 5.8 ms at 96 MHz.
  Hybrid ≈ 6.2 ms + 3–9 ms paging at 144 MHz (45–75 %), ≈ 3.1 + 3–9 ms at
  288 MHz. The codec clock floor stays High for Opus.
* **Firmware:** `status` gains `dec: n= us= max_us= loads=` — this
  track's decoder calls, µs inside them (sum, largest) and pager line
  loads inside them (the decoder's own paging, apart from the loop's).
  RAM: +16 B `.bss`. Resident image 83,508 → 83,204 B (−304: the CRC
  table); code pages 74 → 73; slot 827,392 → 819,200 B.
  `__stack_bottom` 0x132300 → 0x1321e0 (main stack +288 B).
* **Stack:** the Opus decode chain's frames (from the firmware's
  prologues): `OpusDecoder::decode` 748 B, `celt::decoder::decode`
  1,660 B, `quant_partition` 532 B, `mdct_backward` 220 B.
* Not done: hybrid still pages (needs ~22 fewer code lines or a larger
  code cache); the playback loop's own lines per frame are unmeasured
  (the `dec:` counters separate them on the device); no device run.
* **Device test plan (lead):** see the AU-8 report; files
  `/.orpheus/test/t_opus.opus`, `/.orpheus/test/t_opus_p.opus`.

### 2026-11-24 — AU-5 (gapless): join code clustered in `hot.toml`, per-codec open, read-ahead 256 KiB before the end (worker, no hardware)

From the lead's runs of `d7a2cf4`: the preparation runs (15–16 ms, 53–58
loads), but the joins did not get shorter. The MP3 open went from
14 ms / 39 loads (`ec43e38`) to 34–42 ms / 118–135 loads; FLAC stayed at
42–47 ms / 136 loads. The first FLAC join ran without preparation, with
745 frames queued (`u=1`). The lead allowed `hot.toml` changes for this.

* **`hot.toml` cluster `gapless-join`** (firmware only): the paged code of
  the join window and of the preparation packed together from the start of
  the code window, 38 sections, 12,302 B (13 lines). It holds:
  * the hand-over (`Ring::hand_over`, `handed_over`);
  * the preparation (`prepare`, `inspect_head`, `orpheus_audio::probe`,
    `orpheus_codecs::inspect`, `WindowSource`);
  * the decoder open (`orpheus_codecs::open` and its MP3/FLAC/WAV arms,
    the decoders' `open`, the MP3 and FLAC head parsers, `Arena`);
  * the DSP format check, the clock floor (`codec_name`, `sound_active`,
    `dsp_clock_floor`, `clock::set_level`), `Player::gapless_next`, and
    the `StreamRing` methods.

  Before, these were scattered over many pages in link order. A join
  touched lines on many pages and evicted the decoding hot set (the
  32-line cache), and the preparation's probe/inspect lines evicted the
  open's. Now the preparation warms the head-parsing lines the open uses
  again, and a join touches few lines in two adjacent pages. Not included
  because they are single large sections: `play_file` (10.6 KB, mostly
  its playing loop, which is hot), the audio task's poll (28.8 KB,
  `run_player`/`play_item` inlined), and `AnyDecoder::decode` (26 KB,
  hot). `resident.toml` is not touched.
* **The read-ahead for the first join.** `ReaderAtEnd` (`Prefetch`) now
  comes when 256 KiB of the current file are left to read (≈ 2 s of FLAC,
  11 s of 192k MP3), not 64 KiB. A queue's first track shorter than that
  (the corpus parts) asks at its first block. Until the DAC starts, the
  headroom gate counts as roomy, so the read-ahead open and the
  preparation run before the DAC starts.
* RAM/stack: no new statics or future state (a constant, the code order
  and the open split; `__stack_bottom` 0x1322b8 in this build). Frames: audio task
  4,992 B, `play_file` 2,260 B (unchanged); the new `open_*` frames are
  under 1.4 KB (`open_vorbis` 1,348 B, not on a playing path). Resident
  83,500 B (unchanged).
* Not verified on the player (`loads` and `open_us` per join are the
  measure). The lead's previous numbers, for comparison:
  * `ec43e38`: MP3 14.1 ms / 39 loads; FLAC 48.7 ms / 145.
  * `d7a2cf4`: FLAC 46.7 / 136 and 41.7 / 136; MP3 34.0 / 118 and
    41.8 / 135.

* **`orpheus_codecs::open` per codec.** It was one 6 KB function with every
  codec's open inlined, so the cluster carried the Vorbis/Opus/Speex
  set-up too. Each arm is now its own function (`open_mp3`, `open_flac`,
  …, `#[inline(never)]`; no change in behaviour). The cluster takes the
  MP3/FLAC/WAV arms only: 16 → 13 lines.

### 2026-11-24 — AU-5 (gapless): the preparation runs, from the file's head; join statistics per join (worker, no hardware)

From the lead's runs of `ec43e38`: `underruns=0` through both joins of the
FLAC and MP3 splits (three runs, every `ub:` 0). But `prep_us=0
prep_loads=0` on every join, and the FLAC open (≈ 49 ms, 145 loads) leaves
only ≈ 20 ms of the 70-ms ring.

* **Why the preparation never ran.** It probed the next stream inside the
  stream ring. That stream gets bytes there only after the current file
  has been read to its end, and then only as the last ≈ 16 KiB of the
  current file drain. Those 16 KiB are ≈ 0.14 s of FLAC, and they are
  read only while the DAC has headroom. The "≥ 2 KiB buffered" condition
  was therefore met only in the last moments, if at all, and the join
  came first.
* **Now:**
  * `prepare` reads the next file's head itself: up to 8 KiB, through a
    copy of its file handle, into the free end of the decoder arena. It
    probes it, skipping leading tags (a tag longer than the window is read
    past, at most four reads), and inspects it through the new
    `orpheus_audio::WindowSource` (a window of a file with the file's
    positions and length; host-tested).
  * It runs as a chore right after the read-ahead open, independent of
    the ring. At the hand-over the ring's read position moves to the codec
    data the preparation found, and only the decoder opens.
  * The player is asked for the next track (`ReaderAtEnd`) when the
    current file's last 64 KiB are being read (≈ 0.5 s of FLAC, 2.7 s of
    MP3 at 192 kbit/s), not only once it is read to its end. That gives
    the open and the preparation time to find a moment with headroom. The
    next file's bytes still come after the current file's.
* **Join statistics per join.** `status` prints `join#N:` (joins since
  start-up). At each hand-over, the fields that join has not measured yet
  read `-` until they are (open time and loads at the first block,
  underruns after two halves, library and art after 2 s). The read-ahead
  and preparation fields are cleared at the read-ahead open, so they
  belong to the next join. Before, a poll right after a join could show
  the previous join's numbers.
* RAM/stack (`xtask build`; base = main `7acdb74` in this worktree):
  `__stack_bottom` `0x1322a0` → `0x1322a8` (main stack −8 B). Resident
  83,508 → 83,484 B. Frames: audio task poll 4,992 B (unchanged),
  `play_file` 2,244 → 2,260 B.
* Not verified on the player. Expect `prep_us`/`prep_loads` > 0 and the
  FLAC join's `open_us`/`loads` lower than 49 ms / 145. The decoder open
  re-reads the stream head (FLAC `open` parses it again), so not all of
  its pages move out of the join.

### 2026-11-24 — AU-5 (gapless): the next stream prepared before the join, the player told after it, exact underrun ledger (worker, no hardware)

From the lead's run of `879d967`: MP3 joins clean. The FLAC split's 2nd join
took 104 ms with 372 page loads (the 1st, 13.9 ms with 38). The open path
had been evicted during part 1, most likely by the UI repaint of the new
title. `ub:` did not add up to `underruns`.

* **Less paged code at the join (option a, as far as RAM allows).** A
  second decoder cannot be built beside the playing one: two FLAC
  decoders need ≈ 2 × 34–38 KiB, and the arena is 62 KiB. So the parts
  that can move out of the join window do:
  * The stream read ahead is **probed (tags skipped) and inspected while
    the current one plays**: `Ring::prepare`, a chore when ≥ 2 KiB of it
    is buffered and the DAC has headroom. It uses the new
    `StreamRing::next_source` (a byte source over the next stream: probe,
    consume, `skip_to` past a tag larger than the ring; host-tested). The
    format and plan travel with the read-ahead, and the hand-over only
    opens the decoder.
  * The **player is told after the new track's first block**:
    `Player::gapless_next` (host-tested against the simulation: it is what
    `DecoderEnded` opens) says which item comes, so the decoder ends,
    hands over, opens and decodes first. `DecoderEnded` + `Opened` (player,
    store image, snapshot code) follow. On every way out before that
    block (another rate, an open error, a stop, a card hand-over) the
    player is told first.
  * The join window is now: hand-over, decoder open, first decode. The
    title, the info snapshot, the library place and the art come later
    (unchanged).
  * `status` adds `prep_us=` / `prep_loads=` (the preparation's time and
    pager line loads).
* **Exact underrun ledger** (`orpheus_audio::Ledger`, host-tested with a
  writer/DAC simulation). Every operation that keeps the writer from the
  DAC is noted with its length. The halves `PcmOut::push` finds missed at
  a half boundary go to the longest operation noted since its last
  boundary, else to the decoder; a boundary with nothing missed forgets
  them. The causes (`ub: join= fill= serve= pf= prep= lib= art= dec=`)
  add up to `underruns` by construction. The old rule charged the time an
  operation exceeded its starting headroom (halves at risk), which
  overcounted.
* RAM/stack (`xtask build`; base = main `a4b4045` in this worktree):
  `__stack_bottom` `0x1321d0` → `0x1322a0` (main stack −208 B: the ledger,
  the prepared stream with the read-ahead and its copy at the hand-over,
  2 statistics). Resident 83,508 → 83,508 B. Frames: audio task poll
  4,992 B (unchanged), `play_file` 2,324 → 2,244 B.
* Not verified on the player. Open: whether the remaining window (decoder
  open + first decode + `play_file`'s set-up code) still pages too much
  after a UI repaint. The next options are a `hot.toml` cluster for the
  FLAC/MP3 open paths (not touched here: the package brief said not to
  edit `hot.toml`/`resident.toml`) or warming those lines by a dummy
  open just before the end.

### 2026-11-24 — AU-5 (gapless): bounded lookups after a join, path-walk cache, underruns by cause (worker, no hardware)

From the lead's run of `0b7ac9a`: the joins are almost clean (`u=0`,
`open_us` 15–22 ms), but `lib_us` ≈ 184 ms and `art_us` ≈ 115 ms each
exceed the whole PCM ring (≈ 70 ms). The first FLAC join started with only
`head=513`.

* **Path-walk cache (`orpheus-fs`, host-tested).** Every open under
  `/.orpheus/…` walked from the root directory. On the owner's card that is
  likely tens of sector reads, and it was common to both lookups. Now
  `Volume::set_walk_cache` lends a `WalkCache` (3 directories, 140 B,
  static in the firmware like the sector cache). A walk starts at the
  longest cached directory prefix. The cache is emptied when a directory
  grows (exFAT lengths), when an entry set is deleted or restored
  (remove, rename), and at each mount. The fs test helpers lend one, so
  the whole fs suite (write ops, power cuts, big directories, 9 formats)
  runs with it. The new `tests/walk_cache.rs` covers hits, growth past a
  cluster, rename and remake, and fails without the growth invalidation
  (checked).
* **Library lookup in steps.** `Library::find_begin` / `find_step`
  (host-tested; `find_near` is built on them) do one probe per call: the
  next row first, then the binary search. The firmware runs one probe per
  chore, one session each, at most three small reads. A folder whose
  track was not in the library (hidden folders such as `/.orpheus/test`,
  which the library leaves out) is remembered (`LibState::miss_dir`) and
  its next tracks are not looked for. A non-gapless start still searches
  in one session (no audio yet).
* **Album art.** A gapless track in the same folder as the one before keeps
  the cover shown (an album). Otherwise there is one cache lookup (path walk
  now cached).
* **Later and gated.** A gapless track's library place and art are looked
  for only after its first 2 s (`META_AFTER_MS`). Its title and info are
  shown once two halves of it are out (no card work).
* **Headroom from the DMA position.** `PcmOut::headroom` = written halves
  not finished + the half being filled − what the DMA has played of the
  current half. It uses the channel's remaining-bytes counter (+0x1C,
  readable while running per U-BT-4; unconfirmed on the DAC channel).
  `ROOMY` = 1.5 halves of real headroom (the old estimate counted the half
  being played as whole).
* **The DAC is fed first near a track's end.** Reads into the next file
  happen only when the DAC has `ROOMY` ahead (`StreamRing::space_in`,
  host-tested). The current file's own reads are never held. UI requests
  wait while the current file's last bytes play (≤ 0.7 s; the read-ahead
  open still goes first). The decoder's `NeedData` path no longer serves
  requests inline.
* **Underruns by cause.** `status` adds `ub: join= fill= serve= pf= lib=
  art= dec=`, the current track's underruns, reset with `underruns`. Each
  operation that took longer than the DAC had queued when it started is
  charged with the halves it made the DAC miss. `dec` is what no operation
  explains (decoder/DSP time, interrupts).
* RAM/stack (`xtask build`; base = main `62b50b4` in this worktree):
  `__stack_bottom` `0x132128` → `0x1321d0` (main stack −168 B: walk cache
  140, cause counters 24, search state). Resident 83,500 → 83,508. Frames:
  audio task poll 4,992 → 4,992 B, `play_file` 2,332 → 2,324 B.
* Not verified on the player. Open: whether the walk cache brings
  `art_us` and each library step under ~20 ms (the `ub:` counters will
  say). If one step is still long, the next lever is keeping the library
  file open across sessions. Also open: whether the DAC channel's remaining
  counter reads correctly (if not, `head` reads one half high; the
  join-start `head` in `status` would show it).

### 2026-11-24 — AU-5 (gapless): join underruns, `playdir`, join statistics in `status` (worker, no hardware)

* **The "fourth track"** of the lead's run is most likely the **Shuffle
  setting**: Files → OK queues the folder with `shuffle || setting`, and a
  shuffled queue of three started at part 0 plays `0, 2, 1` or `0, 1, 2`
  depending on the seed. The polled frame counts fit `p0, p2 (32,303 and
  86,447 are the same track), p1 (ends at 70,916)`, and the earlier run
  (ends at 89,167) fits `p0, p1, p2`. The underrun counter is per track (reset
  before each `play_item`, so it covers that track's join), which is why it
  read 6 twice during part 2. No player bug was found on the path
  (`Prefetch` is issued only for `next_pos`, `CancelPrefetch` re-reports the
  reader's end only after a cancel). `status` now shows `q=POS/LEN shuf=0|1
  title=…`, so the next run can tell.
* **Join underruns: likely cause and changes.** After the hand-over, the
  new track's first stream refill also ran the queued UI request and then
  the gapless track's library lookup (`Library::find`: a binary search of
  ~13 probes × 2 reads over 5,740 tracks, plus a session open) and its
  art-cache lookup. Each of these does card I/O while nothing feeds the
  DAC, and the PCM ring holds at most 70 ms. Changes:
  * All card work besides the stream refill is now a **chore** that runs
    only while the PCM ring holds ≥ 1.5 halves (≈ 52 ms; at most 300 ms
    late), one per decoded block. In order of priority: the read-ahead open
    (`Prefetch`; it was done right when the reader reached the end), a
    waiting UI request, the gapless track's title + player snapshot +
    library place, then its art.
  * The new track's title and info are published only after its decoder
    runs (`Ring::shown`): the UI's repaint no longer competes with the
    decoder open for the four-slot code cache.
  * `Library::find_near` (host-tested): it tries the previous track's
    row + 1 first. Album tracks are consecutive rows, so it is one read
    instead of the binary search.
  * There is no card read before the next decoder opens when ≥ 4 KiB of it
    is already in the ring.
* **Measuring on the device.** `status` reports the last join as
  `join: head=` (PCM frames queued when the decoder ended), `open_us=`
  (decoder end → first block of the next track), `loads=` (pager line loads
  in that window), `u=` (the new track's underruns after its first two
  halves: the join's), `lib_us=` / `art_us=` (the two meta chores),
  `u_meta=` (underruns after them), `pf_us=` (the read-ahead open). If
  `u > 0` with `open_us` > the headroom (`head` / 44.1 frames per ms), the
  open itself is too slow (paged code: `loads`). The next step would then be
  probe + inspect of the next stream at read-ahead time. If `u_meta > u`, a
  meta chore is too long even when roomy (the next step: the library
  binary search in steps).
* **`playdir PATH [START]`** (developer channel, idle loop): queues a folder
  as Files → OK does (same `PlayReq::PlayDir`, index order), but always with
  shuffle **off** so tests are repeatable. `PATH` may also name a file (its
  folder, from that file); `START` is a file name in the folder. Replies
  `playdir ok`; progress is in `status`. HIL check in
  `docs/plan/07-tooling-testing.md` §5.4.
* RAM/stack (`xtask build`; base = main `ec797b0` in this worktree):
  `__stack_bottom` `0x132070` → `0x132128` (main stack −184 B: join
  statistics 40 B, `Ring::shown` ≈ 112 B, chore state in the audio task's
  future). Resident 83,468 → 83,500 B (+32, a few short literals). Frames:
  audio task poll 4,992 → 4,992 B, `play_file` 2,332 → 2,332 B.
* Not verified on the player (I have no hardware). Open: the join numbers
  from the next run; Vorbis (codebook set-up in the open window); whether
  0 underruns needs the pre-inspect step.
### 2026-11-24 — VER-4 part 1: fuzz targets, committed corpora, findings (worker, no hardware)

* **16 new cargo-fuzz targets**, one project per crate (`crates/<crate>/fuzz/`,
  `sim/fuzz/`), bodies in the project's `src/lib.rs`, each crate's stable
  `tests/fuzz_corpus.rs` replaying `tests/regress/<t>/` and
  `tests/fuzz-corpus/<t>/` on every `cargo test`: `otp_uri`, `otp_vault`,
  `bt_frame`, `bt_tx`, `usb_scsi`, `usb_devchan`, `proto_records`,
  `settings_mount`, `index_open`, `library_open`, `player_queue`,
  `player_resume`, `sd_registers`, `input_samples`, `power_samples`, and
  **`ui_keys`** (the apps over the simulator: keys, timing, system events,
  settings with any value; oracles include golden-consistent redraws).
  Invariants per target: `docs/verification/baseline-2026-11.md` §8.1.
* **Corpora committed** for all 30 targets: 13.5 k files, 15 MB, every file
  ≤ 64 KiB, 8 dictionaries; seeds from the crates' encoders and fixtures and
  the synthetic conformance corpus (`cargo xtask test fuzz-seeds`). Every
  new target fuzzed 10 min after its last fix, every old target 5 min (ASan),
  corpora `cargo fuzz cmin`-ed; codec/picture corpora re-merged with
  `-max_len=4096` to stay under 20 MB.
* **Findings** (§8.4): fixed F-BT-1 (a sink's "no absolute volume" report
  set the player's volume to the maximum), F-SET-1 (settings written over
  non-erased bytes after the log end), F-USB-1, F-IDX-1 (letter jumps past
  the listing from damaged index/library files), F-LIB-1 (u32 offset
  overflow in library queries), F-LIB-2, F-PLY-1, F-PLY-2 (repeat one asked
  to open item 0 of an empty queue), F-UI-2 (content, menus, dialogs drawn
  over the volume overlay/toast), F-UI-3 (remaining time a second off), and
  the VER-5 LeakSanitizer leak of the simulator (`Volume::into_parts`).
  **Open**: F-UI-1 (volume overlay corners compound; needs a UI decision and
  re-blessed goldens), F-SET-2 (`remove` on a full store returns `Full`),
  F-UI-4 (one more Now Playing redraw difference, reproducer in
  `tests/regress-open/ui_keys/`).
* **Tooling**: `xtask test fuzz-seeds`; `xtask test fuzz-smoke` runs every
  target of every project from the committed corpora and dictionaries;
  `xtask cover --fuzz-only`; `fuzz_dirs` scans `sim/`.
* **Coverage** (§8.3): fuzz only ≈ 0 → regions 78.04 % of the files the
  fuzz binaries contain (48,171 / 61,729); host + fuzz 72.16 % → **83.27 %**
  regions (74,194 / 89,102), lines 83.37 %, branches 80.81 %.
* Not done (part 2): plateau runs (≥ 24 h per target), targets for
  `orpheus-apps` `settings::decode`/`FileRef::from_bytes`, `orpheus-audio`
  ring behaviour, the pager traces, the host tools. The UI target runs at
  1–10 executions/s (a simulator per input, library builds ≈ 0.35 s).
* `docs/verification/findings.md` was not created (this worker's tools
  refused new Markdown files): the findings table is §8.4 of the baseline.
* Gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, `cd fw && cargo clippy
  --workspace -- -D warnings` pass; device steps not run (host-only
  package).

### 2026-11-24 — UI-6a: the Bluetooth app (worker, no hardware)

* **Screens** (`orpheus-apps::screens::bluetooth`, ids 27–30): Home →
  Bluetooth (enabled in the firmware's `app_available`): header, Bluetooth
  on/off, state row (Off / Starting… / Not connected / Searching… /
  Connecting… / Connected / Sending audio / Not responding, with the status
  icon), the connected device on its own row (OK = Disconnect? / cancel),
  Pair new device… (starts the inquiry), Paired devices (count), Reconnect
  automatically (0x0603). **Pair new device** (`BtScan`): live list, kind
  icon from the class of device (headphones / speaker / phone / computer /
  generic; major class is in the middle byte whichever the order), name or
  the address, names arriving after the inquiry; "Searching…" while it
  runs, then "No devices found" / "Search again"; OK connects (the inquiry
  is stopped first), Back stops it. **Paired devices** (`BtPaired`): radio
  marks the connected one; OK connects or asks to disconnect; Menu →
  Connect/Disconnect, Forget… (destructive dialog). **Connecting**
  (`BtConnect`): spinner, Back cancels (0x29); success pops back, a global
  toast "Connected · NAME" (also for reconnects; "Disconnected" when a
  connection is lost); failure (module 15 s timeout, refusal, our 40 s
  guard) → dialog "Could not connect" with Cancel / Retry. No rename (no
  such module command, bluetooth.md §3.7), no signal strength (byte 6 of
  0x03 unknown).
* **Service API** (`services.rs`): `BtPhase`, `BtAddr` (`push_text`
  reversed), `BtKind::from_cod`, `BtDevice` (by value, 24-byte name, cut
  on a character), `BtList`, `BtRequest {Scan, StopScan, Connect(addr),
  CancelConnect, Disconnect, Forget(addr)}`, `bt_phase/bt_count/bt_entry/
  bt_request`, events `ServiceEvent::Other { kind: other::BLUETOOTH, value:
  bt_event::{LISTS, PHASE, CONNECT_FAILED, CONNECTED, DISCONNECTED} }`.
  `settings::id::BT_RECONNECT`.
* **`orpheus-bt`**: connect during an inquiry stops it (0x01) and connects
  at its end (cancel before that drops it); names of unnamed devices are
  asked for (0x02) when an inquiry ends; a newly connected device joins the
  paired list with its found name; **the player's volume** (`set_volume`,
  now remembered when not connected) **is sent as 0x26 on every
  connection** before the audio start, and on every change; a sink report
  that differs within 3 s of connecting (BlueZ starts at 0) or our query's
  answer is answered with ours, later sink changes are taken
  (`Output::SinkVolume`), `Output::VolumeAcked` on the acknowledge of
  0x26. `Addr::to_text` prints the bytes reversed (the lead's finding:
  LSB first on the wire). `local_from_ui`/`ui_from_local` (UI 0–30 ↔
  module 0–31, round trip exact). Tests: 34 (new: connect during inquiry,
  cancel before the inquiry stops, connect → 0x26, UI volume round trip).
* **Firmware**: `bt` task requests `StopScan, ConnectTo, CancelConnect,
  Forget`; `LISTS` (found + paired, ≈ 0.5 KB, copied in place when they
  change); `Snap` + phase, counters of connects/fails/drops/list changes,
  sink state; `bt::VOLUME` (atomic) carries the player's volume to the
  task. `Fw`: volume policy (06 §4.2, the lead's measurement: the tap is
  after the DAC volume): DAC held at UI step 30 (−3 dB) while connected to
  a sink that acknowledged 0x26 or reported a volume, back to the
  player's volume on disconnect or when the sink says it has no absolute
  volume; sink changes become the player's volume. `atj2157-hal/dac.rs`
  untouched (the lead's).
* **Simulator**: fake module (`sim/src/bt.rs`: start-up 1.2 s, five
  devices appearing over 2.7 s, one unnamed until after the inquiry, one
  never named, Car Kit and the phone fail to connect, forget, reconnect at
  power-on), `set:bt=off|starting|on|scanning|connecting|connected|
  streaming|dead`, `set:btfound=N`. 91 goldens `bt_*` (15 journeys × dark/
  light × Latin/Cyrillic/CJK + amber), looked at; Russian/Japanese strings
  shortened until nothing important is cut; 7 kanji added to the Unifont
  subset (from an older Unifont, CJK ideographs unchanged). Behaviour tests
  `sim/tests/bluetooth.rs` (9). All older goldens unchanged.
* **RAM** (`xtask build`, this worktree): `__stack_bottom` `0x131c00` →
  `0x131f38` (**+824 B**): bss +608 (`LISTS` 532, `SNAP` and the UI's copy
  +2 × 16, task/`Fw` fields), resident +216 (rodata, panic-location data;
  expect less in the main checkout). `xtask size --check` fails in a
  worktree as before (resident 83,468; base 83,252).
* **Facts from the lead's run** (01-hardware §9 U-BT-2, U-BT-6):
  addresses LSB first; sink volume 0 after connect; connect refused during
  an inquiry; SDP record advertises HID 0x1124; reconnect within ~10 s;
  the feed tracks the DAC volume. `docs/hw/bluetooth.md` is not in a
  worktree (ignored): the note for its §8 Q8 is in the UI-6a report.
* Not done: track metadata (0x31) at track change; "Output" setting and
  muting the wired output while streaming (U-BT-2 open); details page
  (address, profiles); refresh of the paired list read from the module
  after pairing (the local insert stands in); the reference level is not
  measured for clipping (user check 17).
### 2026-11-24 — AU-5 firmware: gapless track changes and queue edits (worker, no hardware)

* **Gapless read-ahead** (04 §2.2). New `orpheus_audio::StreamRing`
  (host-tested): one byte ring holding the current stream and, once the
  reader has read that file to its end, the head of the next one right
  behind it. The decoder sees only the current stream (`ByteSource`/
  `SeekSource`, file offsets); `hand_over` makes the next stream current
  (its offset 0 directly after the old stream's last byte; unread trailing
  tags dropped). A seek in the current stream drops what was read of the
  next one (read again from its start). No panic paths (each new file path
  would land in resident `.rodata`). The firmware's `Ring` wraps it and now
  owns the files too (`Ring::file`, `next_file`: a `File` is 64 B and is no
  longer moved through the RT task's frames).
* **Flow.** `ReaderAtEnd` → player `Prefetch(item)` → the card task opens
  the item (directory lookup, title kept, not shown) between blocks and the
  ring reads it after the current file. `DecoderEnded` → `Open { gapless }`:
  if that item is the one read ahead, the DAC is **not** drained or stopped
  (`PcmOut` now lives in `run_player`, across tracks); `play_item` hands the
  ring over, publishes the title, and `play_file` probes (ID3 skip through
  `skip_to`), inspects and opens the next decoder in the same arena on the
  bytes already there; the DSP chain keeps its state when the format is the
  same (no `apply_sound`, `set_format` is a no-op then). The decoders trim
  encoder delay/padding themselves (LAME/Xing, Ogg granules, FLAC totals),
  so nothing is inserted or dropped at the join. The clock floor is High
  from the decoder's end until the next track's codec sets its own (Vorbis
  codebook set-up while the PCM ring plays out); underruns in that window
  do not count towards the too-slow abort. Album art (cache only:
  `pictures::track_art_cached`, else the folder's cover when it is the same
  folder) and the library place (`find_now`) follow at the next ring
  refill. No save at a gapless change (the DAC runs; the next pause, stop,
  ordinary track change, hand-over or power-off saves).
* **Fallbacks.** Another sample rate: `play_file` returns `End::Again`,
  `play_item` lets the previous track play out (`PcmOut::finish`) and opens
  the same item the ordinary way (DAC restarted at the new rate). A file
  read ahead that cannot be opened: the same (it then fails as usual). A
  track change the user makes (Next, Prev, jump, new queue, bookmark) is
  unchanged: stop, flush, fresh start. `CancelPrefetch` (repeat/shuffle
  change, queue edit, sleep after track) drops the read-ahead and reports
  `ReaderAtEnd` again so the new next track is read ahead.
* **Host test of the hand-over** (`orpheus-codec-conformance`):
  `join::decode_gapless` is the firmware's sequence (16 KiB `StreamRing`,
  512-B-minimum reads, prefetch when the reader is at the end, probe → tag
  skip → inspect → open in one arena). `tests/ring_join.rs`: a WAV signal
  cut at 1 / 4,093 / 33,333 / 99,999 frames with a `LIST` tag after the
  first part and a 19 KB ID3 tag (larger than the ring) before the second
  joins sample-exact; FLAC×3, MP3×4, 24-bit FLAC×2 and a mixed
  MP3/FLAC/MP3/FLAC chain equal the parts decoded alone. `join::check` (the
  corpus album splits of `xtask test codecs`) now also requires the ring
  path to give exactly the per-part samples: `xtask test codecs --filter
  gapless --quick` (fresh corpus) passes flac/wav bit-exact, mp3 (LAME)
  −118.7 dBFS and vorbis −121.2 dBFS vs the per-track references, all
  220,500 frames = source length. Shifting the hand-over by two bytes
  makes the tests fail (checked).
* **Queue edits** (05 §7.1). `PlayerCmd::PlayNext(file)` (after the current
  track), `AddToQueue(file)` (at the end), `Remove(i)`, `MoveUp(i)`,
  `MoveDown(i)` (Up next; only tracks after the current one) →
  `PlayReq::Edit` → `Server::edit` → `orpheus_player::queue::edit`
  (host-tested): a shuffled queue is first written into the queue in its
  play order (`unshuffle`, in place, 256-bit bitmap), so the order the
  user sees stays and the player goes on unshuffled; the new
  `Command::Requeue { id, len, pos }` updates the player without touching
  the track (prefetch cancelled and asked for again; resume record saved
  with the new queue id unless stopped). Queue id and Up-next generation
  change; `queue.bin` is rewritten at the next save (`Server::queue_dirty`,
  before the resume record in `save_store`). Limit 256
  (`services::QUEUE_MAX`): Files shows "The queue is full" (new string, ru/ja
  in the sim packs) instead of sending. With nothing queued, Play next / Add
  to queue start a one-track queue. The Shuffle setting stays on after an
  edit; switching it off keeps the edited order, switching it on shuffles
  anew.
* `orpheus-player`: `Command::Requeue`, `Player::order`, `queue::{edit,
  unshuffle, Edit, EDIT_MAX}`; 22 tests (+5: requeue plays on gapless in the
  new order, requeue of a shuffled queue keeps the order and the record,
  edits, unshuffle, shuffled edits). The test simulation re-reports the
  reader's end after `CancelPrefetch`, as the firmware does.
* **RAM / stack** (`xtask build`, this worktree, before → after):
  `__stack_bottom` `0x131c00` → `0x131d38` (main stack −312 B: `Ring` +2
  files + read-ahead record ≈ 250 B in the audio task's future, `Server`
  +4); resident 83,252 → 83,252 B (unchanged; `xtask size --check` fails in
  a worktree as before: 83,252 > 81,920, the absolute panic-location paths).
  Frames (`sub sp`): audio task poll 5,024 → **4,992 B**, `play_file`
  2,380 → **2,332 B**, `save_store` 916 → 908 B; new functions all below
  256 B (`Ring::hand_over`, `Server::edit`, `prefetch`, `track_meta`).
* Not done: fades (04 §2.2) around pause/seek/manual skips; saving the
  resume point at a gapless change; making art for a track whose embedded
  picture is not cached yet at a gapless change (the folder's cover or none
  is shown until the next ordinary open); Play next / Add to queue for
  library rows and whole folders (the menus offer them only on Files
  entries); a golden for the "queue is full" toast. Not run on the player:
  user check 18 and the lead's test below.
* Device test plan (lead): `cargo xtask test codecs --gen` makes
  `tests/corpus/gapless/{flac,wav,mp3,vorbis}_part{0,1,2}.*` (a 5-s music
  source cut at frames 60,417 and 131,333). Upload each codec's three parts
  into its own folder, e.g. `cargo xtask dev put
  tests/corpus/gapless/mp3_part0.mp3 /.orpheus/test/gapless-mp3/mp3_part0.mp3`
  (and so on), plus one 48 kHz file into `/.orpheus/test/gapless-mp3/`
  named `mp3_part3_48k.mp3` for the rate fallback. On the player: Files →
  Menu → Show hidden → `.orpheus/test/gapless-mp3` → OK on `mp3_part0`;
  during part 1 and 2 `cargo xtask dev cmd status` must show
  `underruns=0` (the counter restarts per track and covers the hand-over),
  the title changes at each join, `cargo xtask dev stats` shows the clock
  at High briefly and back to the codec's floor. Listening: user check 18.
  Then queue edits: Up next → Menu → Move down / Remove on the next track
  while the current one is in its last seconds (the edited next track must
  follow gapless), Files → Menu → Play next / Add to queue, and 256+ adds
  for the toast; after each edit `cargo xtask dev cmd status` and a
  power-off/on: Music → Resume continues in the edited queue.
| VER-1 (host), VER-5 (host) | **tooling done; baseline measured** | `xtask cover` (line/region/branch, per crate, `--fuzz`, `--html`, `--add`, `--report-only`), `coverage-exclusions.toml` (empty; D-9), `xtask gate --coverage` (report only) / `--m11` (100 % + ASan + Miri), `xtask metrics` → `metrics/*.csv`, `xtask test asan [--lean]`, `xtask test miri`. Baseline: `docs/verification/baseline-2026-11.md` (regions 72.16 %). Not done: corpora (VER-4), conformance corpus inside `cover`, device coverage (VER-6) |
| VER-4 | **part 1 done** | 30 cargo-fuzz targets (16 new, incl. the simulator UI key-sequence target `ui_keys`), committed corpora (`tests/fuzz-corpus/`, 15 MB) replayed on every `cargo test`, 10 fixes + 3 open findings (baseline §8.4), `xtask test fuzz-seeds`, `xtask cover --fuzz-only`. Host + fuzz coverage 83.27 % regions. Not done: plateau runs (part 2) |

## 3. Session log

### 2026-11-24 — VER-1 + VER-5 (host side): coverage, sanitizers, metrics, baseline (worker, no hardware)

* **`cargo xtask cover [--crate NAME] [--fuzz] [--html] [--clean]
  [--report-only] [--add]`** (07 §3.6): host tests built by
  `nightly-2026-11-05` with `-Cinstrument-coverage
  -Zcoverage-options=branch` in `target/cover/`, the toolchain's own
  `llvm-profdata`/`llvm-cov` (installed: `llvm-tools`, `miri`, `rust-src`
  components for that nightly). Only `crates/`, `sim/`, `xtask/` count,
  without `tests/` and `tests.rs`; regions/branches are the union over all
  instantiations and binaries (computed in `xtask` from the JSON export).
  `--fuzz` replays every cargo-fuzz target's corpus files through
  instrumented builds of the fuzz projects. Reports in
  `target/orpheus/coverage/` (CSV per crate/file/function, LCOV, HTML,
  `no-data.txt`, `history/<commit>/`).
* **`coverage-exclusions.toml`** (empty; format documented there and in 07
  §3.6): entries stay unapplied while `approved-by = "pending owner"`.
* **Gate**: `xtask gate --coverage` adds the coverage report without
  failing; `xtask gate --m11` fails below 100 % of non-excluded regions or
  on files without data, and runs ASan and Miri. Default gate unchanged.
* **`xtask test asan [--fuzz-only] [--lean]`** (07 §3.5): workspace tests
  with `-Zsanitizer=address -Zbuild-std` (LSan included), then each fuzz
  target over its corpora under `cargo fuzz run … -runs=0`. **`xtask test
  miri [--crate NAME]`**: the crate list in `xtask/src/sanitize.rs` with
  time boxes and named skips.
* **`xtask metrics [--elf PATH]`** (07 §4.4, columns defined there):
  `metrics/coverage.csv` (per crate, per commit and source) and
  `metrics/static.csv` (resident/bss/stack/pages/slot, 40 largest
  prologue stack frames via `llvm-objdump`).
* **Baseline** (`docs/verification/baseline-2026-11.md`): regions
  72.16 %, lines 71.78 %, branches 65.09 % over 84,088 regions; worst:
  `orpheus-host` 8.4 %, `orpheus-codec-vorbis` 9.6 % (needs the
  conformance corpus), conformance binary 10.7 %, `xtask` 16.1 %,
  `orpheus-codecs` 29.2 %, `orpheus-codec-opus` 37.1 % (needs the RFC
  vectors). No fuzz corpus is committed (only `tests/regress/flac/`), so
  fuzz coverage is ≈ 0. ASan: no memory error in 586 tests; LSan: a
  test-only leak in `orpheus-apps` (fixed) and the simulator's per-`Media`
  FS cache leak (open, needs an `orpheus-fs` API change). Miri: no UB in 14 crates / 232 tests (16 slow or file-touching tests skipped by name; ASan runs them).
* **Fixed**: `orpheus-img` conformance tests raced generating their
  pictures in a fresh target directory (5 failures); `orpheus-apps`
  strings test leaked its packs.
* Disk: the root disk went down to 0.9 GB free during the session (parallel
  workers); one coverage run died with ENOSPC. `--add` (merge a re-run into
  the last profile) and `asan --lean` exist because of that. All
  instrumented build directories were deleted afterwards.

### 2026-11-24 — AU-12 firmware: the music library on the player (worker, no hardware)

* **Flow.** At mount the card task opens the newest complete library
  (`/.orpheus/library/lib-a|b.ydb`). Without one a build is *due* 10 s
  later; after a card-reader session an update is due 5 s after the card
  comes back (unchanged folders are copied without a tag read); Settings ›
  Storage › Rebuild library (`Req::Rebuild`, from the UI's storage action)
  makes it due at once. A due build runs only in the idle loop (no track
  loaded, after a waiting directory-index build) in steps of 32 units (a
  tag read counts 8) on the last 20 KiB of the decoder arena, sleeping as
  long as each step took (≥ 2 ms). Any request, command or player request
  pauses it (`Suspended::pause_in`: buffers flushed, scratch free); the
  idle loop resumes it (a timer wakes the loop when a build becomes due).
  A card hand-over gives a build up (the next build empties its files).
  The UI sees `LibraryState::Building { files }` while there is no library
  yet, `Ready` once one is open (an update keeps the old one usable until
  it is done, then the lists are asked for again).
* **Build state in place.** `orpheus-library` gained `Suspended::new`,
  `step_in`, `pause_in` (the builder's methods now run on a `Run` that
  borrows the state; `Builder` is unchanged outside): the firmware keeps
  the 1,096-B state in the **last bytes of the decoder arena** (no
  playback uses them: Opus 59,488 B + 2 KiB spare leave 4 KiB; the arena
  everything else is lent is shorter by that much; a const assert guards
  it) and drives it where it is — no `Builder` moved through the stack
  (the first version had 2.3 KB / 1.2 KB transient frames for
  suspend/resume and a 1.1 KB static).
* **Card adapters.** `orpheus_index::card::CardIo::new_in` (Storage files
  flat in another folder); `orpheus_library::card::{CardTags, TagState}`
  (TagSource over `orpheus-fs`: `open_dir` with `after` = seek to the
  entry and read past it; `open_file` opens the entry just listed without
  a directory seek). `LetterTable::begin/push` (letter runs from any
  source). Host test (`orpheus-library/tests/card.rs`): FAT32 and exFAT
  images (mkfs), 90 tracks of the mock card written as files, first build
  in 1–7-step sessions with suspends, names identical to a mock-card
  build, `find`, album queue, an update driven in place reads 0 tags,
  only the slot file left, `fsck` clean.
* **Screens → queries.** New `LibraryService` (in `Services`) and
  `LibraryScreen` (screen 26, one per level, resume record = list code +
  focused row id): Music › Artists → `List::Artists` → OK → `ArtistAlbums`
  → `AlbumTracks`; Albums → `Albums`; Songs → `Songs`; Genres → `Genres` →
  `GenreArtists` → `GenreArtistAlbums` → `GenreAlbumTracks`; Recently
  added → `Recent` (100 rows). OK on a track: `PlayerCmd::PlayLib { list,
  start: row }` → `Library::queue` of up to 256 tracks around the row;
  Menu → Play / Shuffle (focused container's tracks, or the track list),
  Play all / Shuffle all (an artist's or genre's tracks). Shuffle all:
  `Library::sample` (256, library order, played shuffled); the card walk
  stays as the fallback without a library. Now Playing menu: Go to album /
  Go to artist (when the track is in the library: `Library::find` at track
  open → `PlayerSnap::lib_now`), landing on the track / album. Unknown
  artist/album/genre strings (en/ru/ja). Library lists share the browse
  window (pseudo directory `0xF…`, rows carry kind, row id, duration;
  `tag::LIBRARY`), letter runs go into `media::LETTERS`.
* **Simulator**: `set:library=ready` builds a real library from the card
  image through the same adapters (sessions of 64 steps); 26 new goldens
  `library_*` (root, artists, an artist's albums, album tracks, albums,
  songs, genres, recent, menu, play, Now Playing menu, Go to album / artist;
  dark/Latin and light/CJK), looked at; all 479 older goldens unchanged.
* **RAM** (`xtask build`, this worktree): `__stack_bottom` `0x1320c8` →
  `0x132600`, main stack 21,304 → 19,968 B (**−1,336 B**): resident
  +1,248 (rodata +976 — the panic-location paths of 9 newly linked files,
  `orpheus-library` ×5, `orpheus-meta` window/length, `orpheus-index`
  extsort…, ≈ 42 B shorter each in the main checkout, so ≈ −380 B there;
  data +280: `SERVER` +256 = `LibState` with the open `Library`, `PLAYER`
  +24), bss +88 (UI future +80). Expect ≈ −960 B in the main checkout.
  `xtask size --check` fails **in a worktree** (resident 83,244 > 81,920;
  the base was already 81,996 here); 103 paths × 42 B ⇒ ≈ 78.9 KB in the
  main checkout.
* **Stack frames** (`sub sp`/`subw`): audio task poll 5,088 → 5,024 B,
  UI task poll 6,016 → 5,984 B. New: `library::step` 2,036 (the builder's
  scan with `orpheus_meta::Tags` inlined), `Run::sort` 1,100,
  `Library::open` 1,340 (mount/hand-back only), `CardTags::open_dir`
  1,044, `publish_list` 860, `fill_queue` 628, `begin` 620,
  `fill_sample` 580, `load_letters` 508, `session` ≈ 320, `pause` 116.
  Deepest new RT path ≈ audio poll + step + `Out::put` → `CardStore::
  write_at` 640 → `Inner::slot` 2,500 → FS open ≈ 11.6 KB, below the
  playing-and-serving path (≈ 13.6 KB) — check the stack high-water mark
  during a first build.
* Not done: an update at every mount (a card changed in a computer while
  the player was off is only picked up by Rebuild or a card-reader
  session); Playlists; "Rebuild" is always an update (never from
  scratch); Ogg/MP4 tags (names from files/folders); no build progress
  in percent (the row says "Building library…"); a build that fails is
  not retried until the next mount, card-reader session or Rebuild.

### 2026-11-24 — BT-2 device side: UART driver, Bluetooth task, shutdown on power-off (worker, no hardware)

Continued an interrupted worker's WIP (merged, reviewed, mostly kept).

* HAL `uart` (new): open/close (reset pulse, clock gate, `BR`, `CTL`),
  error flags, CPU FIFO read/write, TX by DMA (vendor channel-9 word
  `0x00228200`, `TFER` cleared before/after, wait for `UTBB` — VERBATIM
  bluetooth.md §2.3; bounded, aborts on timeout, counts a missing
  completion interrupt), RX `DmaRing`: single-byte DMA with reload into a
  ring, write position from the remain counter (U-BT-4). No UART
  interrupt (an RX-interrupt ring was written and dropped again for its
  resident RAM; it is the next fallback).
* BSP `bt`: pins (GPIO38 TX / GPIO39 RX function 10, pads `0x302A` /
  `0x312A` VERBATIM, GPIO20 enable, GPIO31 host-busy low), `up`/`down`
  (enable line last up / first down, pads parked low when off as the
  vendor does), `Listen` (both UART pads as pulled-up inputs, nothing
  driven, edges counted while the module starts: board side of U-BT-1).
* `orpheus-bt`: `driver` (sans-I/O glue: `Port` trait, power on/off,
  bounded power-off shutdown, stats, the enable line re-checked against
  the state after every drain); transmitter `shutdown` (0x24 once, no
  resend, refused unless the module is up; no restart afterwards); the
  command queue is now a 264-byte ring of encoded frames (was 8 × 81 B);
  TX frames are sent straight from the command layer's buffer;
  `MAX_FOUND` 8, `MAX_NAME` 24, 12 outputs (RAM). The crate has no panic
  sites left in the firmware (their absolute-path location strings sit in
  resident `.rodata.str1.1`: ~0.6 KB in a worktree, ~0.3 KB in the main
  checkout). Tests: 30 (shutdown with/without acknowledge, power-off while
  starting, dead module, start-up/scan/connect over a mock UART, a noisy
  byte-by-byte line, the queue ring, lost enable output).
* Firmware `bt` task (main executor): module off at boot (GPIO20 driven
  low); the Bluetooth setting powers it 5 s after boot, never in a host
  test; setting changes switch it; headset keys → player; playback
  start/stop → command 0x05; state and device name → `BluetoothService`;
  a newly connected device is saved (settings key `0x06F0`) as the
  reconnect target. `Ui::power_off` sends 0x24 when the module is up and
  waits at most 1 s for its completion (owner decision 2026-11-24); a test
  run reboots before that point, so `bt shutdown` exercises the same
  sequence by hand. No other restricted command is sent anywhere.
* Developer channel (`xtask dev cmd "bt …" --wait N`): `probe` (on,
  start-up exchange, full report, off again; ≤ 25 s), `on`, `off`,
  `status`, `scan`, `connect N` / `connect pN`, `disconnect`, `listen`
  (≈ 4 s), `shutdown`, `rx dma|cpu`, `tx dma|cpu` (module off only). The
  report: state, identity, pad words of GPIO20/31/38/39, UART `STA`/`CTL`,
  ring remain counter and peak, RX bytes/frames/skipped/bad length/bad
  checksum, TX frames/bytes/failures/DMA timeouts/missing completion
  interrupts, command counters, paired and found devices, the last 24
  bytes received.
* Diagnostics text uses `t!` statics so it is paged (plain byte-string
  literals go to the resident mergeable string section).
* RAM (`xtask build`, worktree): `__stack_bottom` `0x131728` → `0x132018`,
  stack −2,288 B (bss +2,136: driver 1,328, task 336, ring 256, request
  channel 60, snapshot 40, UI task +88; resident +160). Over the ~2 KiB
  aim by ~240 B; levers: ring 128 B (−128), fewer found devices.
* Not run on the player. Resident additions: none (no new handler; the
  DMA completion handler is `atj2157_hal::dma::*`). `check-resident` ok.
* Open: U-BT-1 (board side), U-BT-4, U-BT-5 (01-hardware §9); volume
  policy, metadata at track change, the pairing screens and U-BT-2 are the
  rest of BT-2.
### 2026-11-24 — AU-14 device: Pictures and album art (worker, no hardware)

* Started from an interrupted worker's unbuilt WIP (merged
  `worktree-agent-aeef1db8813ec5d0a`). Kept: `PictureService`
  (`show_picture`/`picture_state`, `PicReq`/`PicState`, `data::PICTURE`)
  replacing `PlayerService::art`, `PlayerState::art` (a generation), the
  Files screen's `BrowseMode` (Files / Music › Folders / Pictures with
  `BrowseFilter::images_only`), the viewer screen, the strings,
  `thumb::ART` at 52 × 52 and `ART_BIG` (128 × 128), `lookup_into`,
  `Gfx::to_screen`, `Shell::overlay_shown`. Changed: the viewer's search
  (no scan budget that could stall), a dropped request (`Pending` after
  `Data(PICTURE)`) redraws and asks again, `Busy` re-checks every tick
  and retries when the music stops, fixed light/white text on black in
  every theme (was unreadable in Light); Now Playing marks the art when
  it arrives even if the clock changed meanwhile (the WIP lost it), Big
  art no longer repaints every second (only the 2-px progress line; the
  overlay is drawn over the art in the frame buffer, the art is asked for
  again only when the overlay goes), battery/minute/volume/USB events no
  longer force a full redraw. Dropped: `scale::rgb888`/`feed_rgb565`
  (unused).
* `orpheus-index`: `Spec::images_only` (filter bit 4; index files of the
  Pictures listing are their own `…-n4.idx`; older files still parse).
* `orpheus-img` (host-tested): `art::cover_rank` (`cover`, `folder`, `front`,
  `album`, `AlbumArt…` with a picture extension), `art::Tee` (one decode,
  two scalers), `CacheKind::meta` (an entry's metadata without reading
  its pixels), `Orient::quarter_turns`.
* Firmware, new module `pictures` (card task). **Pixels go straight into
  the UI's frame buffer**, never into a RAM copy: a screen asks while it
  draws; after that frame is on the panel the UI lends the frame buffer
  (`Req::Picture(PicJob)` with its address and the box) and draws nothing
  until `tag::PICTURE` comes back; the screen then marks the box. The UI
  takes a job back that has not started (any key but volume, or 3 s:
  e.g. the card is with USB) through `PIC_SLOT` (compare-and-swap on the
  job number, so a taken-back or replaced job never starts) and cancels a
  running decode with `PIC_CANCEL`. A request made under a menu, dialog,
  toast or the volume overlay is not sent; the screen is redrawn (and
  asks again) once the overlay is gone. A lost data-ready tag is noticed
  within a loop (`PIC_SLOT` back at 0).
* **Decoding schedule.** Picture files decode only while no track is
  loaded, in the idle loop, with the whole 64 KiB arena and the stream
  ring as the source, clock floor High, sleeping 1.5 ms after every 8 ms
  of work (thread mode and the RT beat keep running). While a track is
  loaded (playing *or paused*) only ~2 KiB of the arena are free: the
  viewer shows "Stop playback to view pictures" (chosen over small-only
  decoding: even BMP/QOI need the scaler's 3 KiB) and retries by itself
  when playback stops. **Album art** is made when a track opens, before the
  codec takes the arena: cache lookup (`/.orpheus/cache/art`, 52 × 52
  `Axxx.YTC` and 128 × 128 `Bxxx.YTC` buckets) keyed by the track's
  locator, size and mtime → else the embedded front cover (ID3 `APIC`,
  FLAC `PICTURE` via `orpheus-meta`; tag reader and tags placed in the
  arena) → else the folder's best `cover/folder/front…` picture (cached
  under the cover file's key; looked for once per folder). One decode
  feeds both sizes (`Tee`); the big copy only for JPEG/BMP/QOI (PNG and
  GIF need the arena), retried small-only if the arena is short. Misses
  are recorded as a 0 × 0 entry so the next open does not look again.
  Showing art is a cache read into the frame buffer (5.4 KB small, 32 KB
  big), served between ring refills while playing; Big art without a big
  copy shows the small one centred on black. The art is never resident.
* Pictures on Home (`app_available`); Files opens pictures in the viewer
  too (next/prev skip non-pictures).
* Simulator: `PictureService` decodes for real (`orpheus-img`, 64 KiB
  arena) from a second read-only mount of the card; the card has
  `Aurora.png`, `Lake.bmp`, `Sunset.qoi` (generated, `sim/src/pictures.rs`,
  part of the card image's cache key) besides the two damaged stubs.
  22 new golden images (`pictures_*`: list, menus, PNG, BMP, QOI
  letterboxed, rotated, filled, damaged, busy; dark/Latin and light/CJK),
  `media_files_notice_*` now opens `/Images` (pictures open the viewer).
* RAM (`xtask build`, `__stack_bottom`, this worktree): main stack
  23,768 → 21,696 B (−2,072). Resident rodata +1,760 B: the panic-location
  paths of 14 `orpheus-img`/`orpheus-meta` files (≈ 1.35 KB here; each path is
  42 B shorter in the main checkout, so expect ≈ −1.5 KB there) plus the
  new strings in the merged string section; `.data` +144 (`SERVER`: art
  key, folder art, generation; `PIC_RESULT`), `.bss` +160 (UI future +128,
  picture statics). Audio task future unchanged (6,192 B). Paged: code
  52 → 62 pages (`orpheus_img` 45 KB, `Decoder::step` alone 28.5 KB;
  `orpheus_meta` 11.5 KB), rodata 14 → 16; slot 626,688 → 724,992 B.
  Deepest new RT paths: art from the cache while playing nests ≈ 2.2 KB +
  file system under `play_file` (the existing browse path ≈ 1 KB);
  `Decoder::step` 776 B + IDCT 352 B. Check the stack high-water mark
  after Big art during playback.
* Not done: thumbnail grid and its cache, 100 % with panning, rotation
  remembered per file, wallpaper, delete, Details screen (a toast),
  animated GIF, unsynchronised ID3 pictures, Vorbis/Opus embedded art,
  art in the mini player.

### 2026-11-24 — AU-12 firmware: the directory index in the card task (worker, no hardware)

* `orpheus-index` additions (host-tested): module `card` — `Storage`,
  `DirSource`, `NameSource` over a `orpheus-fs` volume. The traits are
  synchronous, `orpheus-fs` is async: each call is polled to completion
  with the caller's task `Context` (`card::spin`; the SD driver's timers
  register with the card task's waker). Index files live in
  `/.orpheus/index/X/DDDDDDDD-SF.idx` (X = last hex digit of the directory:
  16 shards keep name lookups short with thousands of folders), merge
  scratch in `/.orpheus/index/sort-a|b.tmp` (or `sort-c|d.tmp` for a build
  that runs while another is suspended); directories created on first
  use; open files cached per session (`Handles`) and closed at its end.
  `Index::read_letter_runs`, `LetterTable<N>` (letter runs in RAM, 4 B
  each, answers `letter`/`seek_letter` exactly like the index). Tests:
  build in slots that suspend/resume and close their files, a second
  build on the other scratch pair in between, query, find, rebuild after a
  change, `fsck` clean, mounted with the `/.orpheus` write prefix (FAT32
  and exFAT images made with `mkfs`); letter table vs index on random
  folders.
* Firmware flow (`media.rs`): every `Browse` makes the usual counting pass
  and folds `Signature::add` into it. Then `Index::open(dir, spec)`; if it
  is current the listing is served from it at once (`sorted: true`, rows
  → names via `open_locator`, the window around the focused file, letter
  table loaded). Otherwise: idle card task and ≤ 128 entries → built at
  once (second scratch pair, no unsorted flash); else the on-disk listing
  goes out (`sorted: false`) and the folder becomes the background build
  target (one at a time; a new target replaces the old one, whose partial
  file and scratch files are removed before the next build starts).
  Background building (`audio::build_slot`) runs only in the idle loop:
  steps of 64 entries/rows on the last 16 KiB of the decoder arena, a
  sleep as long as the step took (≥ 2 ms, thread mode keeps ≥ half the
  CPU), suspended (`Suspended` in the server) as soon as any request,
  command or player request arrives. When it finishes and the folder is
  still shown, the sorted listing is published (`Data(BROWSE)`, focus
  kept). Windows of a sorted listing come from the index rows. The play
  queue follows the listing's order when the folder's index is current.
  Card hand-over drops the index state and removes a build in progress.
* Names come from the directory entries through the index rows'
  locators (one `open_locator` per row): the alternative, a full
  directory pass per window, costs the whole directory for every window
  of a large folder.
* `BrowseService::focus_hint` (new, default no-op; Files calls it when the
  focus moves or is restored): the card task publishes the window around
  that file when a listing becomes sorted, so `position()` finds it.
  `Fw`: re-browses when the filter changes (sort order, hidden files),
  asks again for the rows the screen wants when the listing arrives or
  becomes sorted (they were dropped before while the listing was
  incomplete), letter jump from `media::LETTERS` (80 runs; larger tables,
  e.g. date order of a big folder, fall back to the loaded window).
* RAM (`xtask build`, `__stack_bottom`): stack 22,976 → 21,464 B
  (−1,512 B). `SERVER` +536 B (build state with `Suspended` ≈ 320 B, two
  directory cursors, the current `Index`, orphan name), `LETTERS` 336 B,
  focus hint 12 B, UI task future +32 B, audio task future unchanged
  (6,192 B; the 840-B `DirEntry` of a build step lives in it, not on the
  stack), ≈ 600 B resident merged strings — the panic-location paths of
  the new `orpheus-index` files (absolute paths: shorter in the main
  checkout than in a worker's worktree). Paged code +4 pages
  (`orpheus_index` 17.4 KB). Stack frames of the new sync paths: build step
  ≈ 0.9 KB + file system, window from the index ≈ 0.9 KB + `Index::rows`
  0.5 KB (served also during playback; check the stack high-water mark).
* Not done: the quick stamp (an index is only used after the counting
  pass confirms it, as before for the on-disk listing); `Index::find` for
  the focus (a linear `position_of` pass instead); several abandoned
  builds in a row leak all but the last partial file until that folder is
  built again; Music library index, playlists, Favourites (rest of AU-12).

### 2026-11-24 — AU-5 firmware: orpheus-player in the card task (worker, no hardware)

* The card task (RT executor) owns a `orpheus_player::Player` (in the
  `Server` static, built in a non-inlined function). UI commands arrive as
  `media::PlayReq` on a new `PLAY` channel, checked after every decoded
  block (and waited on while paused); media requests (`REQS`: browse, text,
  the new Up-next window, card hand-over) are still served between
  refills. Power-off moved to `PlayReq::Shutdown` so it is seen within a
  block. The decode loop feeds `Opened`, `OpenFailed`, `Position` (every
  block, source frames), `Tick` (1 s), `ReaderAtEnd`, `DecoderEnded`,
  `DecoderFailed` (10 corrupt frames, 8 underruns, failed seek, ring too
  small), `Drained`, `Shutdown`, and carries out `Open`/`OpenTrack`,
  `Seek` (decoder seek; the stream ring is now a `SeekSource`), `Pause`/
  `Resume` (DAC stopped while paused), `Stop`. `Prefetch` is not carried
  out: the next track opens after the current one played out (no gapless).
  No fades yet.
* Files on the card (formats in `orpheus_player::image`, host-tested):
  `/.orpheus/queue.bin` (magic `YQU1`, count, queue id = CRC-32 of serial and
  locators, serial, 8-byte locators, CRC-32), written when a folder is
  queued; `/.orpheus/player.bin` (588 B: resume slots A/B, 64 B each, then the
  bookmarks, 460 B, magic `YBM2`, each value with its own CRC), written in
  place (only the changed span) at pause, stop, track start, card
  hand-over and power-off — never while audio is being output (a card
  write can outlast the PCM ring), so there is no 60-s save while playing.
  Both are loaded after mount; the resume track's title is resolved and
  offered as Music → Resume.
* `orpheus-player` changes: getters (`len`, `queue_id`, `ab`, `sleep_timer`),
  bookmarks carry the sample rate (for their time in the list; `Player`
  624 → 752 B), `SleepAfterMs(0)` also cancels "after this track", a
  shuffled `PlayQueue` picks a seed whose order starts at the chosen item
  (so every item plays once), `StoreImage`/`Loading` (resume slots only; the
  bookmarks are encoded from the player when the file is written), queue
  file encode/decode. 18 tests (4 new).
* RAM (`xtask build`, `__stack_bottom`): stack 24,752 → 22,968 B
  (−1,784 B). Of that, +1,796 B is resident MP3 seek code
  (`Mp3Decoder::find` 1,142 B, `record` 166 B, more `parse_head`, rodata),
  resident through the `orpheus_codec_mp3::*` rule — it could be paged by
  excluding those symbols in `resident.toml` (not mine to edit). Data and
  bss net −16 B: the player (752 B) and resume slots went into `Server`,
  the text-viewer buffer (1,536 B) moved out of it into the decoder arena
  (the decoder now leaves 2 KiB of the 64 KiB arena free while it plays:
  largest need 59,488 B still fits), the two DSP scratch buffers became one
  (−1 KiB), Up-next and bookmark windows (2 × 208 B, 2 × 456 B), `PLAY`
  channel 144 B. Audio task future unchanged (6,192 B); `play_file` poll
  frame 2,276 → 2,372 B; UI task future +128 B.
* Open: on-device test (user check 13 and the lead's test plan); gapless
  hand-over (reader into the next file, decoder re-open in the arena);
  fades; periodic resume saves while playing; queue edits / Play next /
  Add to queue / Favourites / Shuffle all; "Resume at start-up" setting;
  OK-hold Stop clears the resume point as 05 §7.1 says (the player keeps
  its current track, so OK on an idle Now Playing still restarts it).

### 2026-11-24 — CI green; website published (lead)

* GitHub Actions Gate, Docs and Site all pass (runner fixes: test-image
  tools installed; exfatprogs 1.2 lacks `mkfs -q/-P` and `dump.exfat -s/-d`
  → those cross-checks run only from 1.4; ImageMagick 6 `convert` as well
  as 7 `magick` for the picture fixtures).
* The owner turned GitHub Pages on (legacy branch build); switched it to
  the project's Site workflow (`build_type=workflow`, repository variable
  `PAGES_ENABLED=true`) as planned in 09 §3. **Site live at
  https://timschmidt.github.io/orpheus/** (D-11).

### 2026-11-24 — the radio works on the player (lead)

* Merged FM-1 fix `ebc145a5` (SRAM3 blocks chained; the step counter had
  overwritten the radio's state). On the player with earphones as antenna:
  `fm on` → chip 0x5804, RSSI 39 at 87.5 MHz; `fm scan` → 13 stations
  (92.9/26, 104.7/22, 100.7/20, 90.5/19, 99.1/19, …). Listening (stereo,
  volume, RDS) is user check 20. Re-checked: `acc read` at the moved SRAM3
  address (1,018 mg, `present=1`, 0 I2C errors); `fm on` during music stops
  the music and starts the radio (chip 0x5804, RSSI 37).

### 2026-11-24 — user check 10 (test tones) done by the owner

* Owner, earphones: left/right/both and the 8 level steps as described.
  **Open issue AU-0b**: faint clicking/ticking at regular intervals, maybe
  also in music; heard on the Rockbox port too but not with the stock
  firmware. Suspects: PA settings the vendor uses and Orpheus does not
  (`PA_CTL` bit 27, bit 10 at enable; 01-hardware/audio.md §2.4), or
  periodic activity coupling into the amplifier (NOR page loads, SD
  reads, LCD/backlight PWM). Next: look for periodic transients in the
  post-PA Bluetooth capture (null-sink method) with a pure tone, and
  correlate their period with refill/paging/LCD timing.

### 2026-11-24 — M7: radio, steps, tools merged; device smoke test (lead)

* Merged FM-1/UI-5a (radio) and SENS-1/UI-5c/UI-5d (steps, stopwatch,
  timer, alarm). Collisions resolved: screen ids (radio 32–33, steps/tools
  34–37), service event kinds (radio 3, steps 4, tools 5), radio frequency
  setting moved to 0x0D50 (0x0D20 is the step goal), user checks 20–22.
* On the player: boot and `playdir` fine (0 underruns, ≈ 3,180 line
  loads/s, stack free 11,208 B). **Accelerometer works**: chip id 0x13,
  |a| = 1,020 mg at rest, 0 I2C errors, counting at 25 Hz (`present=0` in
  `steps status` although the chip answers — to fix). **Radio does not
  work yet**: tuner register 02 = 0x4000 (ENABLE clear), chip id reads
  0000, RSSI 0, scan finds nothing; `fm on` while music plays leaves the
  radio off. Sent back to the FM worker.
* CI on GitHub: Docs and Site pass; Gate fixed for the runner's older
  exfatprogs (mkfs without `-q -P`, dump.exfat cross-check only from 1.4).

### 2026-11-24 — PWR-5 and four AU-5 fixes on the player (lead)

* Merged PWR-5 (Performance setting, battery logger, `xtask drain report`,
  `play-block` hot cluster, logger state in SRAM3) and the AU-5 fixes. On
  the player: `playdir` plays (no reboot, 0 underruns, stack free 8,192 B);
  **browsing a large folder during 48 kHz MP3: 0 underruns, `serve=0`**
  (was 8–14); `playdir B` while A plays switches queues; the card
  **remounts** after a card-reader session with the host still holding it;
  the battery logger writes its first row after a minute
  (`/.orpheus/test/drain-N-label.csv`); Opus 0 underruns (≈ 2,650 line
  loads/s). Open: MP3 playback pages ≈ 20 % more than before (≈ 3,220
  against ≈ 2,670 line loads/s, 24.6 against 18.4 loads per decode call,
  0 underruns); the MP3 frame code is inlined into the paged
  `AnyDecoder::decode` despite `resident.toml` (≈ 7 KiB to make it
  resident) — to revisit with the hot-set work. Bit-rate display fix
  host-tested only. User check 19 (unplugged battery runs at Normal and
  Maximum) is ready for the owner.

### 2026-11-24 — AU-8: Opus plays on the player (lead)

* Merged AU-8 (`cceac998` Ogg pages > 8 KiB read packet by packet; `730427ff`
  Opus frame code/tables packed for the pager: `hot.toml` clusters
  `opus-celt`/`opus-frame`/`opus-silk`, range decoder out of line, PVQ row
  computed, 64-B CRC table; decoder stats in `status`). On the player:
  `t_opus.opus` and `t_opus_p.opus` (48 kHz, 10 s) play with **0 underruns,
  0 corrupt**, decode 6.75–6.80 s per 10 s at the governor's level.
  Regression: Vorbis, Speex and MP3 CRCs unchanged (bit-identical),
  `playdir` on a 231-track folder plays with 0 underruns, stack free 5,972 B,
  pager ≈ 2,670 line loads/s during 48 kHz MP3 playback. Resident image
  78,828 B. Open: hybrid-mode Opus still pages (≈ 20 lines too many).

### 2026-11-24 — queue editing on the player; BT-3 offline analysis; new bugs (lead)

* **Queue editing works on the player**: with a 231-track folder queue
  playing, Files › file › Menu › Play next → 232 items and toast "Plays
  next"; Add to queue → 233 and "Added to the queue"; order and shuffle
  unchanged.
* **BT-3**: the vendor host firmware has no path to the module's HID
  service (no sender of `0x32`/`0x2D`, `0x27` dead code, no HID strings or
  settings; `05 16/17` are play/pause notifications). The module's HID SDP
  record is unchanged after the `0x32` restarts. Remaining options: the
  safe experiments E2/E4/E5/E6 of `docs/hw/bt3-offline-2026-11-24.md`
  (passive UART logging while the host drives HID; `0x27` in each link
  state; a `0x05` sub sweep; an unknown-id scan with "all keys released"
  payloads), then module firmware work only with the owner (D-6).
* **Bugs found (open)**:
  - Browsing Files during 48 kHz MP3 playback: 8–14 underruns per track,
    all charged to `serve` (audible dropouts while browsing).
  - After `usb msc ro` with the host still holding the mount ("busy") and
    `usb dev`, the firmware reported `card: remount failed` (fresh boot
    mounted fine).
  - A watchdog reboot (no fault record, RT executor in `Timer::after`)
    after `playdir` during playback followed by an unknown dev command;
    `playdir` is ignored while a track plays.
  - Now Playing › Info shows "Bitrate 0 kbit/s" for a 48 kHz MP3.

### 2026-11-24 — AU-5 gapless closed for now; PCM spill not adopted (lead)

* Three attempts at a PCM spill (extra decoded audio lent from the decoder
  arena across the join) were measured on the player and **not kept**:
  `24f07c5` (never engaged; slower joins), `2ef18a6c` (joins clean, 13–18 ms
  opens, `u=0`, but 1–6 underruns per track at the hand-back), `d306da44`
  (no hand-back; joins clean but 6–13 underruns per track from `fill`/`dec`
  in steady state). Main stays at the `28d4520` behaviour.
* **Known issue (AU-5)**: gapless joins are clean for MP3 (0 underruns over
  three runs); for FLAC the **first** join after a queue starts opens in
  ≈ 71 ms with ≈ 27 ms queued → 3–4 underruns (an audible tick); later FLAC
  joins are clean. Ideas recorded for later: a smaller spill only at the
  join with the decode cost measured, or a lighter FLAC open path.
* Queue editing (Play next / Add to queue / Remove / Move) is merged but not
  yet exercised on the device (HIL in VER-7).

### 2026-11-24 — headphone jack into the workstation mic input; gapless prep (lead)

* The owner connected the player's headphone output to the workstation's
  microphone input. The audio bring-up test then played its tones clearly
  (amplifier fix confirmed on the jack; right-only phase correct, volume
  steps ≈ 5 dB per step visible) but showed large transients, and the
  **firmware failed at every boot**: log `fw: start (paged)`, `audio: dac
  up`, then `Card::open` never completed and the watchdog rebooted to ADFU
  (also with the previous build `ec43e38`). The SD and keys bring-up tests
  passed with the cable; unplugged, the firmware booted at once. Cause
  (inferred): direct-drive output, virtual-ground return shorted to the
  USB host's ground. Safety rule added to 07 §7, fact to 01 §9. Start-up
  log lines added in the audio task.
* Gapless `d7a2cf4` on the player: preparation now runs (15–17 ms, 53–60
  loads), joins 2–4 clean, but FLAC join 1 had 1 underrun (no preparation,
  745 frames queued) and the MP3 open regressed to 34–42 ms / 118–135 loads
  (was 14 ms / 39). Sent back with permission to cluster the join path in
  `hot.toml`.

### 2026-11-24 — AU-5 gapless: MP3 and FLAC joins clean on the player (lead)

* `ec43e38` (next stream prepared before the join, player told after the
  first block, exact underrun ledger): `playdir` on the corpus album splits,
  three runs — **0 underruns through both joins of FLAC and MP3**, all `ub:`
  causes 0. Joins: MP3 30.8 ms / 105 loads and 14.1 ms / 39 loads; FLAC
  ≈ 48.7 ms / 145 loads with ≈ 28 ms queued at the old decoder's end (thin
  margin). Open: the preparation step never ran (`prep_us=0`) and the join
  statistics are published one join late — sent to the worker. Queue
  editing is not yet tested on the device.

### 2026-11-24 — AU-5 gapless: MP3 joins clean on the player (lead)

* Merged `879d967` (bounded lookups, path-walk cache, by-cause underruns).
  `playdir /.orpheus/test/gapless-mp3`: **0 underruns over all three parts**
  (join head 1,617 frames, re-open 13.9 ms / 38 loads, lookups 0 µs).
  FLAC: first join clean (13.9 ms / 38 loads), second join paging-bound
  (104 ms / 372 loads → 4 underruns); the `ub:` per-cause counters do not
  add up to `underruns` yet. Both sent back to the AU-5 worker.

### 2026-11-24 — PWR-5 (D-4) part 1 on the player; gapless fix measured (lead)

* **Max level (288 MHz, VDD 0x18, SYSCLK bit 8, NOR ÷6)** works: clock
  self-test (`dev ram clocks --test clocks`) PASS — 192 MHz 72,912 µs and
  288 MHz 48,608 µs against 291,669 µs at 48 MHz (exact scaling), RAM and
  NOR DMA CRC ok, VDD code 0x18 read back. Turbo (192 MHz) verified for the
  first time too. Full firmware pinned at 288 MHz (`gov off`, `clock 288`;
  `VOUT_CTL` = 0x32E75118): FPU benchmark 1,603 µs vs 3,087 µs at 144
  (1.93×), DSP chain 1,827 vs 3,549 µs (1.94×), 30 s MP3 decode 4.99 s vs
  7.61 s (1.52×, paging-bound), Vorbis 3.63 s vs 5.86 s (1.61×); decoded
  output bit-identical to 144 MHz (CRC) for MP3 and Vorbis; Speex fine.
  **Opus still fails at 288 MHz** (one file needs a larger stream ring,
  the other is "decoder too slow"): Opus is memory/paging-bound, not CPU-bound.
* **Battery drain cannot be measured on the cable**: with USB connected the
  battery stayed at 4,144–4,148 mV through 4 min of 288-MHz playback (the
  player runs from VBUS; the charger had already terminated, `CHG_CTL`
  0x8C410738). Next: a battery logger (voltage, level, state per minute to
  `/.orpheus/test/`) and a user check with the cable unplugged for equal
  periods at Normal and Maximum. Test command `charge pause|resume` added.
* **Gapless fix (AU-5 `0b7ac9a`) measured with the new `playdir`**: order
  correct (`q=` 1/3→3/3, `shuf=0`; the earlier "fourth track" was the
  Shuffle setting); decoder re-open 15–22 ms with 52 ms headroom → 0
  underruns at the join itself, but the library lookup (≈ 184 ms) and art
  lookup (≈ 115 ms) after the join cause 4–6 underruns per track, and one
  FLAC join started with only 11.6 ms of PCM queued. Sent back to the AU-5
  worker (incremental/skipped lookups, full ring before the hand-over).

### 2026-11-24 — D-1, D-4, D-6 decided by the owner

* **D-1**: MIT OR Apache-2.0. Added `LICENSE-MIT`, `LICENSE-APACHE` (the
  standard Apache text), `THIRD-PARTY.md`; the Opus and Speex port crates
  now declare `BSD-3-Clause` (Xiph originals), README licence section.
* **D-4**: the 288 MHz / 1.3 V level is to be tested, verified, its battery
  drain characterised, documented and kept as an option → new WP PWR-5.
* **D-6**: module firmware work gated for now; ask again once the other
  development is complete.

### 2026-11-24 — D-10 decided by the owner

* The presenter remote is a Bluetooth remote (the module's keyboard +
  consumer HID reports). The tilt (IMU) pointer is built only if BT-3 finds
  a Bluetooth transport for mouse data; otherwise SENS-2 and the pointer
  parts of UI-6b are dropped (no USB pointer, no module firmware work).

### 2026-11-24 — published (D-11, owner's decision)

* The repository is public as `github.com/timschmidt/orpheus` (branch
  `main`; worker branches stay local). Before publishing: tracked files and
  history checked for secrets, flash dumps and vendor material (none; the
  vendor-derived `docs/hw/`, `docs/vendor/` and `local/` stay ignored per
  D-7); a stray 5.6 MB test recording (`44100`, committed by mistake in the
  Bluetooth volume commit) removed from main's history; root-level audio
  files now ignored. Licence (D-1) is still open, so the code is public but
  not yet under an open-source licence.

### 2026-11-24 — AU-5 gapless/queue merged; first device run (lead)

* Merged AU-5 (gapless, queue editing). Fixed the `orpheus-img` "fixture
  race": test threads of one binary shared `gen-<pid>`; fixtures are now
  generated once per process (`OnceLock`), verified from an empty folder.
* On the player (Bluetooth off): the corpus MP3 album split from Files →
  OK on part 0 — part 0 clean, then **6 underruns after the first join and
  1 more later**, and a fourth track with part 1's length appears to play
  after part 2. Gapless is therefore **not yet working on the device**; sent
  back to the AU-5 worker with a request for a `playdir` developer command
  and queue position/title in `status` for HIL tests.

### 2026-11-24 — UI-6a merged and tested on the player; Bluetooth volume policy changed (lead)

* Merged UI-6a (Bluetooth app). On the player: Home → Bluetooth → on →
  "Starting…" → automatic reconnect → "Connected · fedora #2" (screenshots
  checked).
* **Test harness fixed**: `pw-record --target bluez_input…` can silently
  record the wrong thing (the BlueZ input is a playback *stream* in
  PipeWire). Reliable capture: a null sink (`pactl load-module
  module-null-sink sink_name=orpheus_bt_cap`), `pactl move-sink-input` of
  the `bluez_input` stream into it, `parecord -d orpheus_bt_cap.monitor`.
  This also keeps test tones off the workstation's speakers. BlueZ as sink
  starts the transport at volume 0 and accepts no absolute volume from this
  module (`EOPNOTSUPP`); the harness raises it locally to 127.
* **Volume policy changed** (06 §4.2 as built): the module acknowledges 0x26
  whether or not the sink scales, answers 0x25 with 128 for a sink without
  absolute volume, and echoes the sent value back as a sink-volume event —
  so whether a sink scales cannot be told. With UI-6a's fixed-reference
  scheme the volume keys had **no effect** on such a sink (measured: −26.5
  dBFS at every volume). Now the DAC always plays at the player's volume,
  the sink is sent full scale (127), and sink volume events no longer move
  the player's volume (following the echo would jump to full scale).
  Measured: DAC 0x6F → −53.6 dBFS, 0x57 → −62.6 dBFS, 0 → silence. Revisit
  with a real headset (user check 17).
* Developer command `bt vol N` (module-scale volume the task sends).
* Key names for `xtask dev keys` are `volup`/`voldown` (not `vol+`).

### 2026-11-24 — plan: M12 documentation/website/branding; presenter remote with tilt pointer (lead, owner's request)

* New plan document `docs/plan/09-docs-website.md` and milestone **M12**
  (BRAND-1, DOC-2…4, WEB-1…3): concise docs for users and developers,
  open-source project files, GitHub Pages site built from the repository,
  Orpheus in the browser (the real UI/app/codec crates on the simulator back
  ends, compiled to WASM), a WebUSB browser installer with backup-first
  safety rules, and one Orpheus/lyre brand generated into firmware, docs and
  site. DOC-1 moved there; REL-1 depends on M11 and M12. New decision D-11
  (public repository name, Pages address, when to publish).
* **Presenter remote and IMU (tilt) mouse back in scope**: 06 §4.3 rewritten
  (staged HID transport — the module's SDP already lists HID 0x1124; the
  tilt-pointer engine with clutch, relative/absolute/scroll modes, one-euro
  filter, jerk rejection, trace tests), 05 §7.12 Remote app (Connect,
  Present with timer and slide counter, Pointer, profiles for PowerPoint,
  Keynote, Google Slides, Impress, PDF, Media, Custom), WPs BT-3
  (investigation, now with stop/go), BT-4, SENS-2, UI-6b; decision D-10 for
  the fallback transport (USB HID, reduced remote, or module firmware).
  PLAN.md requirements, limits and non-goals updated. Note: the running
  Bluetooth-screens worker's package is UI-6a (it was briefed as "BT-3 (UI)").
* Disk: the workstation's disk was at 953 MB free; build caches of the main
  checkout and finished worktrees removed (6.8 GB free afterwards).

### 2026-11-24 — Bluetooth end to end with a host A2DP sink; amplifier gain fix (lead)

* The workstation's Bluetooth adapter served as a test headset: a BlueZ
  agent (auto-accept, discoverable for a limited time) and PipeWire's A2DP
  sink, recorded with `pw-record`. On the player (test mode): `bt on`,
  `bt scan` found "fedora #2", `bt connect 0` paired and connected (no PIN),
  `bt on` later auto-reconnected to it, the module streamed SBC; the module
  appears to the host as `F4:4E:FD:06:7A:20` "ZS285AH_TARNS" with A2DP,
  AVRCP, HFP, **HID (0x1124)** and PnP records.
* **Bug found and fixed: the analogue output was practically muted.**
  `dac::power_up` never set the PA volume field `PA_CTL[14:12]` (vendor: 7).
  Measured through the module's tap: PA 0 → silence, PA 7 → the 522 Hz test
  tone at the reference level ±1.06 dB (DAC 0 dB) and −53.6 dBFS at the UI
  volume (DAC −30 dB) — the tap is after the PA and after the volume
  (U-BT-2 partly answered). This most likely also means the wired
  headphones were near-silent before this build; user check 9 (tones) must
  be made on this build or later.
* Found for BT-3 (sent to that worker): the firmware never sends absolute
  volume (0x26), and the sink starts at 0 → silent stream until it does;
  addresses are shown byte-reversed (`to_text`); connect is refused during
  an inquiry.
* Tools: `xtask dev mem ADDR [N]` reads words from the running firmware over
  the developer channel (`dev peek` switches the player to ymon); PEEK now
  reads aligned requests as 32-bit words (the DAC block reads 0 by byte);
  developer command `pa N` sets the PA volume field.

### 2026-11-24 — plan: final verification stage M11 (lead, owner's request)

* Added milestone **M11 Verification** (08: VER-1…VER-11; 07 §3.5–3.8 sanitizers,
  coverage, adversarial inputs, host UI-sequence fuzzing; §4.4 memory and
  performance history; §5.7–5.9 on-device interaction/fuzz testing, device
  coverage, sanitizers for device code; PLAN.md §4 item 11, decision D-9 for
  coverage exclusions; definition of done extended). Fuzz findings are kept
  in the corpus permanently.
* RTC: measured against the host, 0 s off after 30 s and after an ADFU
  round trip + reset, and it kept running through a firmware reboot to ADFU,
  a RAM start and a watchdog reset; the earlier stop is not reproduced
  (open, U-RTC). The status bar now hides the clock while it is not running.

### 2026-11-24 — AU-12 library on the player (lead)

* Merged AU-12 (firmware, library) (worker). On the player with the owner's
  card (5,740 audio files): the first library build ran in the idle loop
  and finished after ≈ 11 min (estimate was 1–2 min: tag reads per file are
  slower than assumed — open: measure per-file cost, consider reading only
  the first 4 KiB for files whose ID3v2 header shows no text frames); the
  UI stayed responsive throughout and the stack never fell below 6.4 KB
  free. Albums lists the folder names; Artists shows only "Unknown artist"
  — correct: the owner's files carry no title/artist/album tags (only
  `encoder=Lavf61.7.100`, checked read-only through the card reader).
* RTC found stopped again after a series of test resets (clock showed
  00:00; `dev time` reported not running); `dev time --set` restarts it.
  Open (U-RTC): which reset path stops the calendar, and show "clock not
  set" instead of 00:00.

### 2026-11-24 — AU-12 library (host part) merged (worker)

* `orpheus-library`: on-card music library under `/.orpheus/library`
  (two slot files, header written last; incremental updates from per-folder
  signatures; tags read with a budget, ≈ 5.7 KB / 2.4 reads per file on a
  realistic mix), lists Artists → Albums → Tracks, Albums, Songs, Genres →
  …, Recently added; queues and a uniform Shuffle-all sample. Additions:
  `orpheus-index::ExtSort`, `orpheus-meta` budgeted reads and durations.
  Host: ≈ 52 k tracks/s build, 36 ms unchanged update of 5,000 tracks;
  ≈ 200 KB on the card per 1,000 tracks; permanent build state 1,088 B.
  Not yet in the firmware; estimate for the owner's 5,740 files: ≈ 33 MB of
  tag reads, first build 1–2 min (per-file seek cost unknown). Ogg/MP4 tags
  not read yet (names from files/folders).

### 2026-11-24 — project renamed to Orpheus (lead, owner's request)

* "Orpheus" → **Orpheus** everywhere (docs, plan, code, configuration,
  USB strings, build id `orpheus-fw`, xtask state name); crates `orpheus-*`
  → `orpheus-*` except `yoton-bsp` (board support for the Yoton YM03 —
  owner's choice); `orpheus_*` symbols, linker symbols, section names and
  environment variables (`ORPHEUS_BLESS`, …) renamed; `tools/orpheus-ld`,
  `tools/udev/99-orpheus.rules`, output directory `target/orpheus/`. Not
  renamed: the hardware ("Yoton YM03"), the reference repository
  (`/home/tim/Documents/GitHub/yoton/`, `yoton.py`, payloads), the on-flash
  magics (`YST2`, `YFW1`, … — stage 1 in sector 0 checks `YST2`), and this
  checkout's folder name (`…/GitHub/yotonrs`; renaming it is up to the
  owner).
* Card folder `/.orpheus/` → `/.orpheus/` (owner's choice: move the old
  data). The firmware renames it once at mount (write guard lifted for that
  single rename via the new `Volume::set_write_prefix`); on the player the
  queue, resume record, indexes, art cache and test files moved over.
  First version remounted twice and gave `mount` a 10.5 KB stack frame (744 B
  of stack left after boot); now a single mount.
* Logo: a lyre (owner's request) — `docs/logo/orpheus-lyre.svg` (README)
  and the 24×24 `LYRE` icon on the boot/About screens (goldens re-blessed).
* Stack: `Fw`, the settings store and `Ui` (≈ 5 KB) now live in statics
  built by non-inlined constructors; the UI task's poll frame went 6.0 →
  4.7 KB and the stack grew to 27.8 KB (the task future shrank). On the
  player after boot + browse + play + view changes: 7.2 KB free (was 2.8
  KB). The audio task's 5.1 KB frame is the next candidate. The
  orpheus-img test fixtures race when generated fresh by several test
  binaries at once (passes when run alone).

### 2026-11-24 — implementation session 3 (lead)

* Merged AU-14 (device): Pictures app and album art. On the player: Home
  shows Pictures; Show hidden → `/.orpheus/test`: a 1600×1200 JPEG letterboxed,
  a truncated JPEG shows "Cannot show this picture", BMP and PNG render, Down
  rotates by 90°. Album art on the player: an MP3 with an embedded JPEG cover
  (uploaded to `/.orpheus/test/`) shows it in Now Playing's Big art view; the
  first open of a track with new art costs underruns (9 here) while the
  cover is decoded — open item: decode art without stopping the DAC feed, or
  before the previous track ends. The owner's album files showed no art
  (they may have none embedded).
* The "garbled" scrolling text in screenshots was a screenshot artifact: the
  frame buffer is read in 1 KiB pieces while the UI keeps redrawing the
  marquee row. The UI now holds drawing while a screenshot is read (≤ 300 ms);
  four shots of a scrolling row are clean.
* Shuffle all: walks the card's visible folders (no recursion, depth ≤ 8)
  with reservoir sampling into the 256-item queue, then plays shuffled. On
  the player: 5,794 entries / 5,740 audio files in ≈ 27 s, UI responsive,
  then playing. The walk is capped at half the CPU (sleep as long as each 8
  entries took); a fixed 1 ms per 16 entries starved the UI into the
  watchdog after 2,061 entries (found with walk counters in spare
  persistent words). 27 s is slow: the library index (AU-12 rest) replaces
  it.
* **Second NOR-write crash fixed**: LLVM had inlined `nor::program_page`
  (its busy-polling loop) into the paged `NorRegion::program`, so the
  resident-root rule protected only the unused out-of-line copy; toggling
  Show hidden crashed at a line boundary (UsageFault UNDEFINSTR at
  0x231000). All NOR entry points are now `#[inline(never)]`; no paged code
  calls `nor::status` any more. `check-resident` cannot see inlined copies:
  any function that must run resident while the chip is busy needs
  `#[inline(never)]` as well as a root.
* Stack: with Pictures merged only 872 B of 23.1 KB stayed free at the peak
  (picture decoding in the card task over the UI). The USB control buffer
  went from 4 to 1 KiB (the host now moves ≤ 1 KiB per control transfer):
  stack 26.2 KB, 3.9 KB free after pictures + playback. Follow-up: shrink
  the UI task (6.0 KB frame) and card-task frames (5.1 KB; 5.7 KB during card
  hand-over).

* Resumed the two interrupted workers from WIP snapshots. Merged BT-2
  (device). On the player: `bt status` (off at boot), `bt listen` (module TX
  pad toggles 13×, RX pad quiet: wiring as documented), `bt probe` (identity
  "ATS2851", 37 bytes / 5 frames, 0 bad checksums, 3 commands acknowledged,
  DMA RX ring works → U-BT-4 answered), `bt on`/`bt off`, `bt scan`
  (inquiry runs; nothing found in 35 s — needs a headset in pairing mode,
  user check 14), `bt shutdown` (0x24 sent and acknowledged, module off).
* Budgets updated for the full layout; A–B repeat and shuffle verified.

### 2026-11-24 — implementation session 2, part 8 (lead)

* On the player: bookmarks (Now Playing menu → Bookmark here; Music →
  Bookmarks lists it; choosing it plays from the position), resume, shuffle
  toggle (the queue then plays out of folder order: 27 / 231 after the
  resumed track), A–B repeat (markers on the bar; elapsed time cycling
  0:12 → 0:15 → 0:09 → 0:12 … between the marks). "Shuffle all" is not
  implemented (opens an idle Now Playing). Sleep timer not yet observed.
* **Fixed a crash in NOR writes** (02 §3.2 rule 5): toggling Shuffle wrote
  the settings store; `nor::status`/`wait_ready` and the command-byte
  constant were paged, so the pager loaded them while the chip was busy
  programming and the CPU executed garbage (UsageFault UNDEFINSTR in
  `nor::status`). Now `atj2157_hal::nor::*` is a residency root (code and
  constants), and program/erase run with interrupts masked (BASEPRI) so no
  fault can reach the busy chip (suspend/resume is U-NOR-2). Five toggles
  during playback: no crash, 0 underruns. Sector erases (settings log
  roll-over) hold interrupts off for tens of ms; the 70 ms PCM ring covers
  it.

### 2026-11-24 — implementation session 2, part 7 (lead)

* Merged AU-12 firmware (worker): Files lists through `/.orpheus/index/`. On
  the player: the root (35 entries) is sorted at once; a 231-entry folder
  shows on-disk order first and re-sorts in the background within ~20 s;
  playing from it, seeking and browsing while playing work.
* MP3 open/seek code (`Mp3Decoder::find`, `parse_head`, `parse_vbr_tag`)
  excluded from residency: resident −2.5 KB, stack 24.9 → 27.4 KB (peak
  ≈ 21.5 KB measured after browse + play + seek).
* Browsing during playback caused up to 6 underruns (counting pass + index
  open between refills), close to the too-slow abort (8). Underruns within
  500 ms after a served request no longer count towards the abort; Opus is
  still detected as too slow.

### 2026-11-24 — implementation session 2, part 6 (lead)

* Merged AU-12 host (`orpheus-index`, worker) and AU-5 firmware (orpheus-player
  driving playback, worker). On the player: Files → folder → OK on track 2
  plays with "2 / 231"; Next → "3 / 231"; OK pauses; `/.orpheus/queue.bin`
  and `/.orpheus/player.bin` written; after a normal reboot Music offers
  "Resume ‹title›", which continues the same track at the paused position;
  holding Next seeks. Fixed: the DAC kept running during a seek (7
  underruns per seek, close to the too-slow abort); it now stops for the
  seek (3 seeks: 0 underruns).

### 2026-11-24 — implementation session 2, part 5 (lead)

* Upload path for test files (user-approved): developer-channel `PUT`
  request (≤ 4 KiB per transfer, buffer in the idle decoder arena),
  `fcreate`/`fclose` commands, `xtask dev put LOCAL /.orpheus/PATH`; writes go
  through orpheus-fs (write prefix `/.orpheus`). ≈ 50 KB/s. Uploaded Vorbis,
  Opus (1 s and 0.25 s pages) and Speex test files to `/.orpheus/test/`.
* Results above; codec clock floors (Vorbis/Opus: High) set by the audio
  task. Found: the output CRC32 (bitwise, per frame) cost a large share of
  48 MHz; now only with `crc on` (MP3 at 48 MHz: ≈ 87 % idle). A `NeedData`
  the full ring cannot satisfy now ends the track instead of spinning. A
  `REQ_PUT` arm compiled as a catch-all binding (missing import) — caught by
  the orpheus-usb tests; `xtask build` does not run clippy, the gate does.

### 2026-11-24 — implementation session 2, part 4 (lead)

* DSP hang solved. Findings on the way: the audio task's poll function had
  a 13.6 KB stack frame (a 4 KB `Chain` built by value inside it, plus
  futures built on the stack); `Chain` is now built in `main` (frame 4.5 KB,
  start-up stack use 23 → 14 KB). A PC sampler in the IRQ dispatcher
  (persistent words `0x137FE4/E8`, copied to `0x137FDC/E0` at boot) showed
  the RT executor in the PCM push loop: at 48 MHz, MP3 + DSP + the paged
  push loop and DSP ramp helpers overload the CPU and starve thread mode
  until the watchdog fires (the chain alone runs at ≈ 20 % on the player,
  `dspt` command; FP inside the RT executor is fine, `fp` command). Fix: a
  96 MHz clock floor while the chain is active during playback (set by the
  audio task, respected by the governor). The DSP also works on a scratch
  copy of the decoder output. Test commands `dsp`, `gov`, `clock`, `dspt`,
  `fp`, `sound`.

### 2026-11-24 — implementation session 2, part 3 (lead)

* Audio hot path resident (`resident.toml`: `orpheus_codec_mp3::*` and the
  DSP stages' `process` functions), stream ring 32 → 16 KiB, decoder arena
  72 → 64 KiB (largest measured need: Opus 59,488 B). MP3 at 48 MHz now
  ≈ 80 % idle (was ≈ 53 % with the MP3 cluster, ≈ 0 % before), 0 underruns,
  all codecs linked; `mp3-decode` hot cluster dropped (resident instead).
* After an underrun the PCM push now waits for the next DMA half, so a
  decoder that cannot keep up still yields (the RT executor no longer
  starves thread mode).
* **Open: DSP chain hangs on the player.** With any single stage active
  (bass, crossfeed or EQ) playback stops inside `Chain::process` (breadcrumb
  5) until the watchdog fires; also with all of orpheus-dsp resident, so not
  paging. The same sequence (`set_format` 44.1 kHz i16 stereo, flat, then
  the stage) runs in milliseconds on the host. Next: run the chain in thread
  mode on the player (needs RAM: start-up uses 23–28 KB of the 32 KB stack,
  which should be found and cut first), check FP in handler mode, compare
  the thumbv7em code of `Chain::run` with the host's. The chain is built in
  and fed by the Sound settings but bypassed (`DSP_ON` false; test command
  `dsp on`).
* `clock 48|96|144` test command.

### 2026-11-24 — implementation session 2, part 2 (lead)

* User decisions: D-2 full layout (done), BT 0x24 in power-off, Opus seek
  tolerance accepted, test files on the card allowed.
* DSP chain wired (UI → `media::Sound` → audio task); `sound` and `dsp
  on|off` test commands; persistent breadcrumb word `0x137FEC` for
  watchdog post-mortems. Found and fixed: unsigned compare in the PCM ring
  wait deadlocked playback after an underrun (DMA looped stale audio).
* With Opus and Speex linked, MP3 decoding thrashed into the watchdog;
  `hot.toml` cluster `mp3-decode` (8 lines) fixes it: 882,000 frames, 0
  underruns, ≈ 53 % idle, ≈ 1,060 line loads/s at 48 MHz. DSP active still
  thrashes (see AU-6); lazy FP stacking switched off (FPCCR.LSPEN = 0) on
  the way, kept as the safer setting for FP in handlers.

### 2026-11-24 — implementation session 2 (lead)

* Paged firmware boots from NOR; Home, Files, Music, Settings on the player.
  Hang hunted down with new STATS counters (pager, UI loop, RT phases, IRQs
  per line): at 48 MHz the Now Playing UI (~35 %) plus MP3 decode with page
  refaults (~56 %) starved the USB task; the governor (96 MHz) fixes it
  (46 % idle while playing from Files). Also fixed: main-stack overflow (Fw
  held 6 KB by value), card-reader buffer lent from the decoder arena (+16 KB
  stack), handlers bound through `rt::bind` placed resident, a browse
  request loop for short folders, watchdog fed by the pager and every
  executor poll (now: watchdog task on RT + main + UI progress).
* **Full-flash layout (D-2, user decision)**: new stage 2 (slots tried in
  order, slot index recorded, stock path removed), firmware finds its slot,
  HAL NOR guard follows the layout, Rust NOR payload, `install-full`,
  `restore vendor`. Sequence run: dump whole chip (`local/nor-before-full.bin`,
  differs from the stock backup only in sector 0 bytes 0–0x1FF and above
  0x762000), stage 2 tested from RAM, installed, booted; firmware written to
  0x1000 (10.2 s), 0x63000–0x7DA000 erased (1,907 sectors, 35.6 s); boots;
  the 544,768-byte image with Opus and Speex (too big for the dual slot)
  boots from full slot A.
* Merged worker results: OTP-1, BT-1, USB-6, AU-5 (`orpheus-player`), AU-7,
  AU-6, UI-3/UI-4, AU-14, AU-8/AU-9 with the Ogg layers unified.


### 2026-11-24 — AU-8/AU-7 Ogg reconciliation (worker)

* AU-7 (Vorbis, on main) and AU-8 (Opus, Speex) each wrote a `orpheus-ogg`.
  Merged into one crate with a shared core and two packet readers:
  * core (`lib.rs`, `seek.rs`): `PageHeader::parse`, `crc_update` /
    `page_crc`, page sync, the generic answer `Next<P>` (`Packet`,
    `NeedData`, `End`, `Corrupt`), and `Bisect` (granule bisection over a
    `SeekSource`, now holding the stream serial; `Bisect::step` replaces
    `PacketReader::seek_step`).
  * `ranges::Reader` (AU-7's, ported onto the core): zero-copy packets as
    byte ranges of the ring, pages larger than the ring, oversized packets
    skipped as they stream past, `peek_packets` for inspection. Used by
    Vorbis.
  * `assembled::PacketReader` (AU-8's, unchanged apart from the module):
    packets copied into a caller buffer, serial selection, chained-stream
    hand-over, gap detection, resume at a page's granule. Used by Opus and
    Speex.
* Vorbis seeking now uses the shared `Bisect` instead of its own loop
  (granule-0 pages are mapped to "start at the first audio packet", as
  before); AU-7's `find_page`/`page_at`, used only by that loop, are gone.
* The `ogg` fuzz target drives both readers over the same input.
* Conformance after the merge (`xtask test codecs --gen`, 226 files, 0
  failed): Vorbis −114.7 … −122.3 dBFS, 23/23 seeks exact on every file,
  album join −121.2 dBFS (unchanged); Opus corpus −88.4 … −138.4 dBFS,
  RFC 8251 vectors all 12 (24 decodes) range-exact, ≤ −128.9 dBFS
  (unchanged; SILK seek tolerance −50 dBFS kept as AU-8 set it, user
  decision still open); Speex −99.9 … −119.6 dBFS, max 4 LSB (within the
  AU-9 figures).
* Code size (thumbv7em, opt-level s, before LTO): `orpheus-ogg` 7.7 KB for
  both readers and the bisection (AU-8's alone was 4.8 KB). `AnyDecoder`
  on thumbv7em: 984 B (Vorbis variant; Opus 784 B, Speex 608 B).

### 2026-11-24 — AU-9 Speex (worker)

* `crates/codecs/speex` (`orpheus-codec-speex`): libspeex 1.2.1 decoder
  (BSD-3, `LICENSE-XIPH-SPEEX`) in `f32` (float build; no `f64`), NB/WB/UWB,
  every sub-mode, vocoder/null modes, DTX, enhancer on (libspeex default,
  as ffmpeg leaves it), intensity stereo, any frames per packet, native
  8/16/32 kHz output. Uses `orpheus-ogg` unchanged.
* Conformance (31-file corpus, references from ffmpeg `-c:a libspeex`):
  −97 … −120 dBFS RMS, max 5 LSB, lengths exact. Length convention: ffmpeg
  trims nothing for Speex (granules start at −look-ahead, every frame is
  output), so neither does the decoder (`StreamInfo.frames` is `None`).
  Seek: skims packets to the pre-roll start (for streams ≤ 1 MiB, else Ogg
  bisection), then decodes 64 frames (1.28 s) of pre-roll; all corpus seeks
  exact or within −80 dBFS. Beyond 1 MiB, noise-excited frames after a seek
  carry different (statistically equal) noise.
* Arena (used = plan; mono and stereo equal): NB 4,872 B, WB 7,768 B,
  UWB 11,048 B of the 12 KiB budget (packet buffer 2 KiB). Code
  (thumbv7em, before LTO): 32.9 KB. Deepest stack ≈ 1.5 KiB.
* Not done: device benchmark, HIL CRC test, `hot.toml` cluster, chained
  streams.

### 2026-11-24 — AU-8 Opus (worker)

* `crates/codecs/ogg` (`orpheus-ogg`): page parsing, CRC (checked before use
  for pages ≤ `MAX_PEEK`, streamed for larger ones), packet reassembly into
  a caller buffer, serial selection, chained-stream hand-over, granule
  bisection seek. Written here because AU-7 had not created one yet: the
  lead reconciles with AU-7.
* `crates/codecs/opus`: port of the libopus 1.5.2 decoder (BSD-3, licence in
  `LICENSE-XIPH`). SILK integer (bit-exact; every libopus build decodes SILK
  in fixed point), CELT in `f32` because the conformance references
  (ffmpeg's libopus, the RFC 8251 `.dec` files) are float builds and the
  M4F has a single-precision FPU; no `f64`, no recursion (`quant_partition`
  runs on an explicit stack), `sqrt` correctly rounded, `exp2`/`cos`
  within 1–2 ulp. `opuscule` was evaluated and not adopted: MPL-2.0 (not
  permissive under D-1) and a by-value decoder state outside the arena.
* Conformance: all 12 RFC 8251 vectors, stereo (`.dec`) and
  phase-inversion-disabled (`m.dec`, RFC 8251 §10): final range equal on
  every packet, RMS difference ≤ −128.9 dBFS, max 1 LSB (SILK-only vectors
  bit-exact). Corpus (`opus/`, 42 files, libopus via ffmpeg and opusenc,
  SILK/hybrid/CELT, 2.5–120 ms frames, CBR/VBR, header gain ±6 dB, picture
  in OpusTags): −88 … −138 dBFS, lengths exact (pre-skip and end trimming),
  surround refused. Seek: CELT/hybrid exact after the 400 ms pre-roll; SILK
  on stationary synthetic tones keeps −57 … −78 dBFS for seconds (its
  integer LPC/LTP state converges slowly), accepted by a separate Opus seek
  tolerance of −50 dBFS in `checks.rs` (a deviation from 07 §3.3 for the
  lead to confirm).
* Arena high-water (stereo / mono): 58.1 KiB (59,488 B) / 42.1 KiB of the
  64 KiB budget (packet buffer 4 KiB; larger packets are concealed).
  Code (thumbv7em, opt-level s, before LTO): opus 117.6 KB, ogg 4.8 KB.
  Largest stack frames: decode chain ≈ 3 KB (no frame above 1.2 KB).
  Host speed (conformance build, overflow checks on): 26–273 ns per sample
  frame, i.e. 80–800× real time.
* Not done: device benchmark (04 §4.3 row), HIL CRC test, `hot.toml`
  cluster, multistream/surround, FEC, Opus in other containers.

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
  * SD: with no card, CMD8 gets "no response" (STATE bit 4). With the
    card: init at the port's clocks, reads at 9.3 MB/s.
  * Stack overflow is silent without the MPU guard (a 64 KiB temporary on
    the stack jumped to address 0): PG-1 must add the guard region.
* Vendor font: `UNICODE.FON` glyph table at offset `0x172C00` of
  `nor_descrambled_8m.bin` (directory entry at `0xFC00`, length `0x210000`);
  33-byte records verified, baseline row 13 (UI worker).
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
| 2026-11-23 | SD sequential read, 64 KiB / 4 KiB requests | 9,294 / 3,432 KiB/s | `hil drivers/sd` |
| 2026-11-23 | Battery raw at rest / charging | 801 / 828 (≈ 3.74 V) | `hil drivers/charge` |
| 2026-11-24 | Opus vs libopus float, RFC 8251 vectors (24 decodes) | range-exact; RMS ≤ −128.9 dBFS, max 1 LSB | `cargo test -p orpheus-codec-opus --release` (`OPUS_VECTORS`) |
| 2026-11-24 | Opus arena high-water stereo / mono | 59,488 B / 42.1 KiB (budget 64 KiB) | `xtask test codecs --filter opus/` |
| 2026-11-24 | Opus code size thumbv7em (before LTO) | 117.6 KB (+ orpheus-ogg 4.8 KB) | `xtask test codec-size` |
| 2026-11-24 | Speex vs libspeex (ffmpeg), 31 files | −97 … −120 dBFS RMS, max 5 LSB | `xtask test codecs --filter speex/` |
| 2026-11-24 | Speex arena NB / WB / UWB | 4,872 / 7,768 / 11,048 B (budget 12 KiB) | same |
| 2026-11-24 | Speex code size thumbv7em (before LTO) | 32.9 KB | `xtask test codec-size` |
| 2026-11-24 | orpheus-ogg code size thumbv7em (before LTO), both readers + bisection | 7,673 B | `xtask test codec-size` |
| 2026-11-24 | Vorbis / Opus / Speex after the Ogg merge | −114.7…−122.3 / −88.4…−138.4 / −99.9…−119.6 dBFS | `xtask test codecs --gen` |
| 2026-11-24 | MP3 48 kHz album file via `dev cmd play`, Home screen, 48 MHz | real time, 0 underruns, main idle 52 %, ≈ 790 NOR line loads/s | `xtask dev stats` |
| 2026-11-24 | Same file started from Files (Now Playing), 48 MHz | main idle ≈ 0 %, decode ≈ 56 %, UI draw ≈ 35 % → USB starved | `xtask dev stats` |
| 2026-11-24 | Same, governor at 96 MHz | 0 underruns, main idle ≈ 46 % | `xtask dev stats` |
| 2026-11-24 | Pager line loads at rest (Home) | ≈ 240/s | `xtask dev stats` |
| 2026-11-24 | Main stack: size / used at idle | 32,192 B / ≈ 14 KB (full image) | INFO stack high-water |
| 2026-11-24 | Watchdog reset after a blocked main executor | ≈ 3 s | `xtask dev cmd hang` |
| 2026-11-24 | Card reader sequential read (write-protected test mode) | 2.3 MB/s | `dd` 128 MiB |
| 2026-11-24 | NOR program via Rust payload / erase of 1,907 sectors | 401,408 B in 10.2 s / 35.6 s | `xtask dev install-full` |
| 2026-11-24 | Firmware image, all codecs (full layout) | resident 54,308 B, 43 code + 16 rodata pages, slot 544,768 B | `xtask build` |
| 2026-11-24 | Stage 2 (Rust, full-layout aware) | 876 B of 1,024 | `cargo build -p stage2` |
| 2026-11-24 | MP3 44.1 kHz, 48 MHz, audio hot path resident | ≈ 80 % main idle, 0 underruns, ≈ 1,250 line loads/s | `xtask dev stats` |
| 2026-11-24 | Main stack after start-up (32.6 KB stack) | 9.5 KB free (≈ 23 KB used); after moving `Chain::new` to `main`: 18.6 KB free (≈ 14 KB used) | INFO stack high-water |
| 2026-11-24 | DSP chain alone (bass, 44.1 kHz stereo) on the player, 48 MHz | 20 blocks × 1152 frames in 97 ms (≈ 19 % of real time) | `dev cmd dspt` |
| 2026-11-24 | MP3 + EQ + bass + crossfeed at 96 MHz | 0 underruns, ≈ 90 % idle | `xtask dev stats` |
| 2026-11-24 | MP3 44.1 kHz at 48 MHz, output CRC off | ≈ 87 % idle | `xtask dev stats` |
| 2026-11-24 | Vorbis q5 44.1 kHz stereo | real time at 144 MHz, 0 underruns | `dev cmd play` |
| 2026-11-24 | Speex WB 16 kHz | real time at 48 MHz, 0 underruns | `dev cmd play` |
| 2026-11-24 | Opus 96 kbit/s 48 kHz | not real time at 48–144 MHz (paging-bound) | `dev cmd play` |
| 2026-11-24 | Upload over the developer channel | 165 KB in 3.3 s (≈ 50 KB/s) | `xtask dev put` |
| 2026-11-24 | Main stack peak after all formats + DSP (28.0 KB stack) | 19.2 KB | INFO stack high-water |
| 2026-11-24 | Firmware with the directory index (AU-12 firmware) | resident 81,844 B, main stack 21,464 B (−1,512), 52 code + 14 rodata pages, slot 626,688 B | `xtask build`, `nm` `__stack_bottom` |
| 2026-11-24 | Firmware with orpheus-player (AU-5 firmware) | resident 80,724 B (was 79,084), bss 122,608 B, main stack 22,968 B (was 24,752), 48 code + 14 rodata pages | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with BT-2 device side (worktree build) | resident +160 B, bss +2,136 B, main stack −2,288 B; `orpheus_bt::driver::Driver` 1,328 B | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with Pictures and album art (AU-14 device), worker worktree build | resident 81,444 B (was 79,532), bss 123,160 B (was 123,000), main stack 21,696 B (was 23,768), 62 code + 16 rodata pages (was 52 + 14), slot 724,992 B | `xtask build`, `nm` `__stack_bottom` |
| 2026-11-24 | Firmware with the music library (AU-12 firmware), worker worktree build | resident 83,244 B (was 81,996), bss 123,088 B (was 123,000), main stack 19,968 B (was 21,304), 71 code + 16 rodata pages (was 66 + 16), slot 802,816 B (was 761,856); audio task poll frame 5,024 B (was 5,088) | `xtask build`, `llvm-nm` `__stack_bottom`, `llvm-objdump` `sub sp` |
| 2026-11-24 | Firmware with gapless hand-over and queue edits (AU-5), worker worktree build | resident 83,252 B (unchanged), main stack −312 B (`__stack_bottom` 0x131c00 → 0x131d38); audio task poll frame 5,024 → 4,992 B, `play_file` 2,380 → 2,332 B | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | AU-5 join fixes (chores gated on PCM headroom, deferred title, `find_near`, `playdir`, join statistics), worker worktree build | resident 83,500 B (was 83,468), main stack −184 B (`__stack_bottom` 0x132070 → 0x132128); audio task poll 4,992 B, `play_file` 2,332 B (unchanged) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | AU-5 bounded lookups (walk cache, stepwise library search, DMA headroom, underruns by cause), worker worktree build | resident 83,508 B (was 83,500), main stack −168 B (`__stack_bottom` 0x132128 → 0x1321d0); audio task poll 4,992 B, `play_file` 2,324 B (was 2,332) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | AU-5 prepared next stream, player told after the join, underrun ledger, worker worktree build | resident 83,508 B (unchanged), main stack −208 B (`__stack_bottom` 0x1321d0 → 0x1322a0); audio task poll 4,992 B, `play_file` 2,244 B (was 2,324) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | AU-5 preparation from the file's head, earlier read-ahead, per-join statistics, worker worktree build | resident 83,484 B (was 83,508), main stack −8 B (`__stack_bottom` 0x1322a0 → 0x1322a8); audio task poll 4,992 B, `play_file` 2,260 B (was 2,244) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |

| 2026-11-24 | Opus decoder alone per 20 ms CELT stereo frame (`music_96k`), before → after AU-8 (Opus) | 799 k → 760 k instructions; 44–46 → 32 code lines, 21–23 → 12 rodata lines; NOR line loads 61.5 (max 161) → 0; faults 258 → 7 | opusbench in unicorn, mapped onto `layout.json`, `orpheus_pager::sim` replay of `pages.bin` |
| 2026-11-24 | Same, other streams after AU-8 (per 20 ms) | CELT 128/256 kbit/s 889 k / 1,175 k instr, 0 loads; SILK WB mono 265 k, 0 loads; SILK stereo 20 kbit/s 430 k, 0 loads; hybrid 24–32 kbit/s 690–744 k, 17–52 loads | same |
| 2026-11-24 | Opus arena high-water stereo after AU-8 | 60,008 B (+520 B PVQ row) | `xtask test codecs --filter opus/` |
| 2026-11-24 | Firmware with AU-8 (Opus), worker worktree build | resident 83,204 B (was 83,508), bss 122,072 B (was 122,056), `__stack_bottom` 0x132300 → 0x1321e0 (main stack +288 B), 73 code + 16 rodata pages (was 74 + 16), slot 819,200 B (was 827,392); clusters opus-celt 25, opus-frame 7, opus-silk 23 lines | `xtask build`, `llvm-nm` |

| 2026-11-24 | Host coverage baseline (host tests + 1 fuzz regression input), regions / lines / branches | 72.16 % (60,678 / 84,088) / 71.78 % / 65.09 % | `xtask cover --fuzz` (+ `--add` re-runs), nightly-2026-11-05 |
| 2026-11-24 | Instrumented coverage build of the host workspace, no debug info | ≈ 0.4 GB build + ≈ 1.1 GB test images in `target/cover/…/tmp` | `du` |
| 2026-11-24 | Coverage after VER-4 part 1, regions / lines / branches | host + fuzz 83.27 % (74,194 / 89,102) / 83.37 % / 80.81 %; fuzz only 78.04 % (48,171 / 61,729 regions of the files fuzz binaries contain) | `xtask cover --fuzz-only`, `xtask cover` (+ fuzz replays), nightly-2026-11-05 |
| 2026-11-24 | Committed fuzz corpora | 30 targets, 13.5 k files, 15 MB | `du --apparent-size tests/fuzz-corpus` |
| 2026-11-24 | Firmware with SENS-1, UI-5c, UI-5d, worker worktree build | resident 83,316 → 83,484 B; `__stack_bottom` 0x132238 → 0x132498 (+608 B); 75 → 79 code pages (+16 rodata), slot 835,584 → 868,352 B; cluster `steps-sample` 3,648 B (4 lines) at 0x222000; sensor task poll frame 68 B, audio task poll 4,992 B (unchanged) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | Step detector on synthetic traces (25 Hz, 8 seeds × walk/slow/brisk/pocket 120 s, run 90 s) | all within ± 5 %; 0 steps: table 10 min, handling, bus 10 min, shaking 3.8–8 Hz | `cargo test -p orpheus-steps` |
| 2026-11-24 | ASan host tests | 63 binaries, 586 tests, 0 memory errors; LSan: 4 binaries (1 fixed) | `xtask test asan --lean` |
| 2026-11-24 | Miri | 14 crates, 232 tests, no UB; full run ≈ 35 min | `xtask test miri` |

## 5. Open questions for the user

* SENS-1/UI-5d (2026-11-24): user checks 20 (200 steps twice, 10 min on a
  table) and 21 (stopwatch against a phone, timer and alarm alerts while
  on). A labelled walking capture (`acc trace`, user check 20 c) would
  replace the synthetic traces in the detector's tests.

* D-9 (coverage exclusions): the list is empty; workers will propose
  entries with `approved-by = "pending owner"` for the owner to review.
* User checks 7–10 in `docs/USER-CHECKS.md` (picture, keys, charge current,
  tones). 8 and 10 are blocking for M4 and M5.
* Decisions D-1, D-3 … D-8 in `docs/PLAN.md` §5 (defaults assumed); D-2 decided (full layout).
* Answered 2026-11-24: Bluetooth shutdown command 0x24 **is** part of normal
  power-off (BT-2 adds it to the power-off path); the −50 dBFS Opus SILK seek
  tolerance is accepted; test files may be added to the card (under
  `/.orpheus/test/`).
