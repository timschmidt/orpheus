# Changelog

All notable changes to Orpheus are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions will
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) from the
first release. The detailed, dated log is [docs/STATUS.md](docs/STATUS.md).

## [Unreleased]

Nothing has been released yet. State on 2026-11-24: Orpheus runs from
flash on the reference player in the full layout (the original firmware
replaced).

### Added

- **Boot and memory**: Rust boot stage 2 that starts the first valid
  slot (an invalid image is skipped) and goes to USB recovery when the boot
  counter trips (a valid image that keeps crashing is switched back from
  the host: `xtask dev switch-slot` or the Install page); demand paging of code
  and constants from SPI flash (1 KiB lines, 8 KiB pages) with a
  profile-guided page packer and a static proof of the resident set;
  watchdog fed only while both executors and the UI make progress; fault
  records and a crash screen.
- **Drivers**: display (DMA), keys, microSD, audio DAC with hardware
  volume, USB device, NOR flash, RTC, charger at the original firmware's
  values, clock levels from 24 to 288 MHz (1.0 V core supply up to
  144 MHz), Bluetooth module UART.
- **Music**: playback of MP3, FLAC, WAV, Ogg Vorbis, Opus and Speex;
  gapless joins; queue with Play next, Add to queue, remove and move;
  resume after power-off; bookmarks; shuffle, repeat, A–B repeat, sleep
  timer; music library (artists, albums, songs, genres, recently added)
  built in the background; album art from tags or folder covers.
- **Sound**: equaliser with presets and two 5-band custom curves, balance,
  bass, crossfeed, channel mode, ReplayGain, volume limit.
- **Files**: sorted browsing (name, date, size) through an on-card
  directory index, letter jump, hidden files, text viewer.
- **Pictures**: JPEG, PNG, BMP, GIF and QOI viewer with fit/fill,
  rotation and slideshow.
- **Bluetooth**: switch the transmitter module on and off, pair, connect,
  reconnect automatically, forget devices, send audio.
- **System**: Home, Settings (sound, playback, display, power, date and
  time, Bluetooth, USB, storage, system), quick settings, lock screen,
  power menu, Performance setting (Normal 144 MHz / Maximum 288 MHz),
  power-off (auto-off and low-battery switch-off are built and await
  checks unplugged).
- **USB**: card reader for a computer; developer channel in every USB
  personality (logs, screenshots, key injection, test commands); a USB
  keyboard personality (boot keyboard; US, UK, German and French layouts)
  — built, not yet tried on the player.
- **USB Drive** (built, not yet tried on the player): an `.iso` or `.img`
  from the card's `Images` folder presented to a computer as a read-only
  CD-ROM or disk, so that it can start from it; files stored in more than
  682 pieces are refused with a reason.
- **Authenticator** (built, not yet tried on the player): time- and
  counter-based one-time codes behind a PIN, with a countdown, typed into
  the computer over the USB keyboard; accounts imported from
  `/.orpheus/otp-import.txt` on the card or sent from the developer
  channel. The PIN only stops casual use (the vault can be copied with a
  USB cable).
- **Tools**: `cargo xtask` for building, size budgets, tests, golden
  images (620), codec conformance, 30 fuzz targets with committed corpora,
  coverage, sanitizers, metrics, device commands, battery-drain reports;
  desktop simulator.
- **Project**: MIT OR Apache-2.0 licence (decision D-1); brand palette and
  assets (`cargo xtask brand`); user and developer documentation; website
  generator (`cargo xtask site`); contribution guide, code of conduct,
  security policy, issue templates and CI workflows.
- **Orpheus in the browser**: the Try it page runs the real screens, file
  system, library, decoders and sound settings compiled to WebAssembly
  (`cargo xtask wasm`), with a synthesised demo card, your own files kept
  in the browser, saved settings and resume point, and a developer panel
  with a key recorder in the fuzz-corpus format.

### Known issues

- The first FLAC gapless join after a queue starts can tick (3–4
  underruns); MP3 joins are clean.
- Making album art for a new track can cause a short dropout.
- Pictures cannot be viewed while a track is loaded (shared memory).
- The clock has stopped after some restarts (set it again).
- Menus are English only on the player; Russian and Japanese run in the
  simulator. CJK file names need the font pack (in progress).
- "Start original firmware" is offered but does nothing useful in the full
  layout.
- Hybrid-mode Opus still causes more flash reads than other codecs.

[Unreleased]: https://github.com/timschmidt/orpheus/commits/main
