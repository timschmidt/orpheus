# Changelog

All notable changes to Orpheus are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions will
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) from the
first release. The detailed, dated log is [docs/STATUS.md](docs/STATUS.md).

## [Unreleased]

Nothing has been released yet. State on 2026-11-27: Orpheus runs from
flash on the reference player in the full layout (the original firmware
replaced) and is used there every day. "Not yet checked on the player"
below means built into the firmware and covered by its automated tests on
the player, but not yet tried by a person; the open checks are in
[docs/USER-CHECKS.md](docs/USER-CHECKS.md).

### Added

- **Boot and memory**: Rust boot stage 2 that starts the first valid
  slot and, when a slot fails three starts in a row, the next one — after
  an update also the old, superseded image (installed and tested on the
  player with a firmware made to crash) — with USB recovery only when
  nothing is left; demand paging of code and constants from SPI flash
  (1 KiB lines, 8 KiB pages) with a profile-guided page packer and a
  static proof of the resident set; rarely used code runs from the
  constants' window in SRAM3, with a trampoline for the few loads whose
  constant shares the instruction's cache line (PG-2), which made room for
  speech and Books; the UI font and merged strings paged (PG-1); watchdog
  fed only while both executors and the UI make progress; fault records
  and a crash screen; a static worst-case stack analysis
  (`cargo xtask stack`).
- **Drivers**: display (DMA), keys, microSD, audio DAC with hardware
  volume, audio input (ADC), FM tuner, accelerometer, USB device, NOR
  flash, RTC, charger at the original firmware's values, clock levels from
  24 to 288 MHz (1.0 V core supply up to 144 MHz, 1.3 V at 288 MHz),
  backlight at the original firmware's levels, Bluetooth module UART.
- **Music**: playback of MP3, FLAC, WAV, Ogg Vorbis, Opus and Speex;
  gapless joins; queue with Play next, Add to queue, remove and move;
  resume after power-off or a restart (the track reopens paused);
  bookmarks; shuffle, repeat, A–B repeat, sleep timer; music library
  (artists, albums, songs, genres, recently added) built in the
  background; album art from tags or folder covers.
- **MIDI and the synthesiser**: `.mid`, `.midi`, `.kar` and `.rmi` files
  play from Music and Files through Orpheus's own synthesiser (band-limited
  oscillators, FM, drum models, filters, chorus, delay, reverb) and its own
  General MIDI sound set; a load governor raises the clock up to 288 MHz
  before it sheds effects and voices. Played by hand on the player: "all
  worked well".
- **Sound**: equaliser with presets and two 5-band custom curves, balance,
  bass, crossfeed, channel mode, ReplayGain, volume limit, key click, fade
  on pause.
- **Files**: sorted browsing (name, date, size) through an on-card
  directory index, letter jump, hidden files, text viewer.
- **Pictures**: JPEG, PNG, BMP, GIF and QOI viewer with fit/fill,
  rotation and slideshow.
- **Radio**: FM with presets, scanning, regions and RDS, the clock set
  from RDS if wanted; the audio takes the analogue path. Works on the
  player; stereo and RDS not yet checked by a person.
- **Recorder** (not yet checked on the player by a person; the microphone
  records there): microphone or radio to WAV, IMA ADPCM or FLAC, automatic
  gain, voice activation, auto split, low cut, marks; Recordings list with
  play and delete.
- **Steps, Stopwatch, Timer, Alarm** (not yet checked on the player): a
  step counter with a goal and a week's history; stopwatch with laps,
  timer with presets, alarms with an alert screen, beep and backlight
  flash (an alarm does not yet wake the player from off).
- **Games**: Blocks, 4096, Solitaire (Klondike), Salvo (an artillery duel
  against the computer or passing the player, with generated terrain,
  wind and drag) and Bricks (a brick breaker); settings, scores and saved
  games. Played by hand on the player: "all worked well".
- **Books**: a library of TXT, Markdown, FB2 and EPUB books with contents,
  bookmarks, Go to and reading settings (fonts, sizes, spacing, themes);
  on the player and on by default since 2026-11-27.
- **Accessibility and speech**: a screen reader for every screen with
  Orpheus's own English formant speech voice (speech rate 0.25×–10×,
  voice, pitch, verbosity, sound cues, music lowered or paused while
  speaking); hold Back + Menu 2 s to switch it on or off, Back + OK for
  "where am I"; text size Large and Extra large, two high-contrast themes,
  hold delay and key repeat settings. In the player's firmware; a
  listening check on the player is open.
- **Bluetooth**: switch the transmitter module on and off, pair, connect,
  reconnect automatically, forget devices, send audio.
- **System**: Home (which apps it shows is a setting), Settings (sound,
  playback, display, power, date and time, Bluetooth, radio, recording,
  USB, language, storage, system, accessibility), quick settings, lock
  screen, power menu, list roll-around in every list, Brightness on the
  original firmware's 11 levels, screen saver, rotation by 180°, keys when
  locked, time zone and clock correction, Power mode (Automatic: the
  cheapest clock that keeps up, up to 288 MHz by itself; Battery saver:
  the lowest clock the peripherals allow; High performance: 288 MHz),
  power-off (auto-off and low-battery switch-off are built and await
  checks unplugged).
- **Firmware updates**: `.ofw` update files written into the slot that is
  not running, checked, and switched to only when complete, safe against a
  power cut at any step; `cargo xtask dev update` (about 40 s on the
  player); Settings › System › Update firmware from the card (not yet
  checked on the player).
- **USB**: card reader for a computer; developer channel in every USB
  personality (logs, screenshots, key injection, test commands); a USB
  keyboard personality (boot keyboard; US, UK, German and French layouts;
  typing works on the player); USB recovers by itself when the computer
  stops being answered (USB-W1).
- **USB Drive**: an `.iso` or `.img` from the card's `Images` folder
  presented to a computer as a read-only CD-ROM or disk; on the player a
  computer read test images byte for byte. Starting a real PC from one is
  not yet checked by a person.
- **Authenticator** (not yet checked on the player): time- and
  counter-based one-time codes behind a PIN, with a countdown, typed into
  the computer over the USB keyboard; accounts imported from
  `/.orpheus/otp-import.txt` on the card or sent from the developer
  channel. The PIN only stops casual use (the vault can be copied with a
  USB cable): [SECURITY.md](SECURITY.md).
- **Sequencer** (simulator and browser only, hidden from Home there; not
  on the player yet): a
  drum machine with 8 drum and 2 synth tracks, patterns, per-step
  parameter locks, songs, live recording and export to WAV and MIDI.
- **Tools**: `cargo xtask` for building, size, stack and residency
  budgets, tests, 1,826 golden images, codec conformance, fuzz targets
  with committed corpora, adversarial-input suites, coverage, sanitizers,
  metrics, a setting-to-effect coverage matrix for every setting, device
  commands, an on-device suite (26 journeys), a PC sampler and fault
  profiler, battery-drain reports; desktop simulator; long soaks on the
  player (`xtask dev soak`) with oracles, time series and minimised
  findings.
- **Project**: MIT OR Apache-2.0 licence (decision D-1); brand palette and
  assets (`cargo xtask brand`); user and developer documentation; website
  (`cargo xtask site`); contribution guide, code of conduct, security
  policy, issue templates and CI workflows.
- **Orpheus in the browser**: the Try it page runs the real screens of
  every app, the file system, library, decoders and sound settings
  compiled to WebAssembly (`cargo xtask wasm`), with a synthesised demo
  card, your own files kept in the browser, saved settings and resume
  point, and a developer panel with a key recorder in the fuzz-corpus
  format.
- **Browser installer** (built; tried against a simulated player only, no
  release published yet): backup-first install, update, restore and
  recovery over WebUSB, with the same code as the command-line tools.
- **Settings › Storage** (not yet checked on the player): Clear caches
  (thumbnails, art, folder indexes), Check card (reads everything, changes
  nothing) and Format card (FAT32 or exFAT in the card's own partition,
  after a question); **System › Diagnostics**, a read-only page.

### Changed

- Settings › System › Performance is gone: Power mode alone decides the
  clock, and Automatic reaches 288 MHz by itself when the work needs it
  (decision D-4 as revised).
- Restart (power menu) and a reboot save the place first, so the track
  reopens paused where it was.
- Playback speed is hidden from the menus until time stretching is built.
- Brightness uses the original firmware's levels 1–11 (default 6); a
  brightness chosen before is not carried over.
- Bluetooth volume: the player always plays at its own volume and sends
  full scale to the headphones (the module cannot tell whether a sink
  scales the volume).
- The settings store holds 192 keys (was 128), in the same flash format.
- Books and the speech voice are in the default firmware.
- Settings › Recording › Source no longer offers Line-in (the player has
  no line input); Settings › Bluetooth › Output is hidden (the headphone
  jack and Bluetooth always carry the same sound).

### Fixed

- The screen reader's speech was choppy on the player (its code paged in
  faster than it played); it is continuous alone, and over music the words
  are (TTS-3b). FLAC clicked many times a second: the clock governor took
  the audio task's decoding for idle time and left the processor at its
  lowest speed; FLAC now plays without underruns, and sources above
  48 kHz get the speed they need from their first block (AQ-0).
- Authenticator › Time check showed local time labelled UTC; it shows
  UTC.
- Music › Shuffle all restarted the player on a large library (the
  watchdog); USB could go deaf until a restart (USB-W1); a test request
  left in memory could make later starts return to USB recovery after
  five minutes; playback spent most of the processor on reloading its own
  code from flash while the interface was busy (e.g. browsing Pictures),
  and the step counter's sensor bus ran at a third of its intended speed.
- Long card operations (cover search, game saves, index lookups) no longer
  starve the interface or cause dropouts; browsing a large folder while an
  MP3 plays has no dropouts.
- The analogue output was nearly muted (amplifier gain never set); the
  microphone recorded silence; the radio's state was overwritten by the
  step counter.
- Recorder › Recordings listed the card's root, and showed a deleted
  recording until reopened.
- A rare flash read error at start-up (the settings store not mounted
  once): thread-mode flash reads now cannot race the pager.
- Idle Home reloaded code all the time (slow keys) because two settings
  lookups shared a cache line; MIDI playback first tripped the watchdog,
  then stopped every song, then underran: now the three test songs play
  to the end without underruns.
- Updates installed from a PG-2 image were too slow and reset by the
  watchdog (the SHA-256 loop ran from the constants' window).
- A damaged Bricks save with a valid checksum could stop the simulator or
  load a broken game on the player; two settings overflowed at the ends of
  their ranges (found by the adversarial suites).

### Known issues

- The first FLAC gapless join after a queue starts can tick (3–4
  underruns); MP3 joins are clean.
- Making album art for a new track can cause a short dropout.
- Pictures cannot be viewed while a track is loaded (shared memory).
- Opening a book while music plays can cause a short dropout, and once
  failed ("The book could not be read."); an EPUB is prepared with the
  music stopped. On a player whose Home choice predates Books, Books stays
  hidden until switched on in Settings › Display › Home screen.
- A faint regular ticking in wired headphones (AU-0b), also heard with the
  Rockbox port, not with the original firmware.
- The clock has stopped after some restarts (set it again).
- Menus are English only on the player; Russian and Japanese run in the
  simulator. CJK file names need the font pack (in progress).
- Storage › Clear caches, Check card and Format card, System ›
  Diagnostics, and Now Playing's Lyrics and Levels views do nothing on the
  player yet.
- Hybrid-mode Opus still causes more flash reads than other codecs.

[Unreleased]: https://github.com/timschmidt/orpheus/commits/main
