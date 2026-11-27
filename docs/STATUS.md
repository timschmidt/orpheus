# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | `docs/PLAN.md`, `docs/plan/`. **D-2 decided 2026-11-24 by the user: full-flash layout** ("wipe the original firmware and take over the full player hardware") |
| Code | Host workspace (`crates/*`, `sim`, `xtask`) and device workspace (`fw/`: `atj2157-hal`, `yoton-bsp`, `firmware`, `boot/stage2`, `payloads/norprog`, `bringup/*`) |
| Player | **Runs Orpheus from NOR, full layout.** Stock firmware and the Rockbox port are erased. Card: the owner's music (exFAT/FAT, 238.3 GB), read only except `/.orpheus/` |
| NOR | Sector 0: Rockbox-port stage 1 (unchanged). `0x001000`: full slot A (Orpheus, 544,768 B). `0x7DA000` settings, `0x7DE000` vault, `0x7DF400` Rust stage 2 (876 B, tries full A, full B, dual slot; no stock-firmware path; the T-6 fallback stage 2 (964 B) **installed and crash-tested 2026-11-24**; since UPD-1 two full slots A (`0x001000`) and B, 2,093,056 B each (image 1,605,632 B on 2026-11-27)). `0x7E0000–0x7FFFFF` vendor data, untouched. Backups: `yoton/dumps/nor_backup_8m.bin` (stock, verified), `local/nor-before-full.bin` (the chip just before the switch), `local/rockbox-nor-0x762000.bin`, `local/stage2-previous.bin`. Way back: `xtask dev restore vendor --i-have-a-backup` (`--verify` compares) |
| Host | Fedora 43, Rust 1.98.1 pinned. Disk ≈ 60 GB free (2026-11-27; worker `target/` dirs deleted after each merge) |
| Current milestone | (2026-11-27) On the player: Music, Files, Now Playing, Pictures, Bluetooth, Settings, Radio, Recorder, Steps, Tools, Authenticator, Games, MIDI, Books, speech and the screen reader (on-device suite 25/26 passing; `books` waits for BOOK-1/2b). Open: M13 measurements (owner's cables), M16 USB audio, M18 jack tools, BOOK-3, UI-6b, the VER packages, REL |

## 2. Work packages

| WP | Status | Notes |
|---|---|---|
| T-1, T-2, T-4a, HAL-0, HAL-1, T-5 | done | see earlier log entries |
| T-3, PG-1 | done | pagelink, PagerCore, check-resident; demand paging on the player (8 KiB pages, 1 KiB lines) |
| PG-2 | **done; on the player** (U-MEM-2, U-DWT-2 answered); speech and, since BOOK-1/2 (2026-11-27), Books in the default image | Cold code runs from the rodata window, which spans all of SRAM3's aliases (48 → 64 pages; MPU region 7 grants SRAM3's RAM half, the stack guard becomes a canary + DWT watchpoint); a trampoline runs an instruction whose constant shares its physical line (`orpheus_pager::bounce`); `resident.toml` `[bin.firmware.cold]`. Player checks in the session entry; U-MEM-2 (SRAM3 execution) is the first |
| PG-2b | **built (host + firmware); causes measured on the player (`bdebff77`); the fixed image is not on the player** (a PG-2 image cannot install another by USB or card: `dev stage` + `dev reset`, owner's OK) | Update data path never cold (SHA-256 round loop straddled its constants' line: 2.5 KB/s, watchdog in the read-back); cold placement pads to a clear line, conflicts back first; synth per-piece constants resident (`hot.toml resident_constants`); cluster `settings-get`; status `clk=`/`cpu=`, `dev stats` names Max; `dev update --stop-after`, `prof --during update` (session entry 2026-11-27 "PG-2b") |
| T-6 | started | Rust NOR payload `fw/payloads/norprog` (erase/program, full-layout guard); ymon and nordump still the reference repo's C payloads. **Stage 2 slot fallback installed and crash-tested on the player (2026-11-24)** (`orpheus_proto::boot::decide`; 964 B; lead's `install-stage2` + crash test: session entry 2026-11-24 "T-6 (stage2)") |
| T-8 | **built (host + firmware), not yet run on the player** | PC sampler (T2, level 0) and pager fault log into the 2 KiB profiler block at `0x307000` (SRAM3), `prof pc\|faults on\|burst`, `prof off\|status`; `cargo xtask dev prof --pc\|--faults S [--during "soak --replay F"] [--only TEXT] [--update-hot NAME]`, `cargo xtask prof DIR`; reports in `target/orpheus/prof/<stamp>/`. Resident +656 B. User/lead check: U-TMR-1 and the Pictures-while-playing profile (session entry 2026-11-25 "T-8") |
| AU-PERF | **built (host + firmware), not yet run on the player** | Playback per block out of the large async functions (`play_block` + small named functions, `Apart` poll boundaries, `orpheus_audio::pcm::Feeder`), codecs' `decode` not inlined, DSP stage mask kept, clusters `mp3-frame` 11 + `play-block` 15 + `play-fill` 6 = 32 lines (no shared physical line; `fits_with` check), 53 % of the measured fault loads in that set or gone; stack on the playback wake-up 6.6 → 1.5 KB; soft I2C timed on T0 (true 100 kHz, was ≈ 30 kHz: sample ≈ 2.7 → 1.0 ms), `acc bus`. Lead: profiles and checks in the session entry 2026-11-25 "AU-PERF"; U-I2C-1. **Up next titles: `seek_dir` per cluster (3,507 → 7 steps per title on 128 KiB exFAT), window keeps its titles (built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: Up next titles cheap". **Chore budgets: every chore runs only when the headroom less a decoder margin covers its measured need, splittable ones stop at the budget (`orpheus_audio::chore`, host model; built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: chore budgets". **Listing pieces reopen the index file from its entry (no path walk after `job:index`), the need counts a piece's fixed cost (built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: listing pieces reopen the index file" **The 55 ms `serve` block: precise `ub=` labels (`ub=serve:picture`, `job:rows`, `req:skip`, …), DAC start forgets the candidate, large overruns always logged, RAM-only pieces do not lower a need, soak reads at every action's end (built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: the 55 ms `serve` block". **`job:index` in stretches of 16 entry sets (`Volume::walk_step`, `CardIo::find_index`); removing a file no longer empties the walk cache (index builds did, and the next lookup searched the root: 52–60 ms) (built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: `job:index` in stretches" **Games saves rewritten in place, a new file's allocation searched in steps of 8 sectors (the 41 ms first piece of every save: 452 bitmap reads from the start; built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: games saves in place" **Games pieces' code in one cluster (`games-io`, 14 lines beside `play-fill` and `dir-walk`; `cargo xtask hot`, a line budget in `stack --check`; built, not yet run on the player)**: session entry 2026-11-25 "AU-PERF: the games pieces' code in one cluster" |
| LAY-1 | done (device) | Full layout installed (`xtask dev install-full --i-have-a-backup`), layout-aware `stage`, `restore vendor [--verify]`. Unifont pack in NOR not done (the UI's fonts are built in) |
| LCD-1, INPUT-1, PWR-2, AU-0 | done pending user checks 7, 8, 9, 10 | |
| SD-1, FM-0, USB-1, USB-3, PWR-1 | done | |
| SYS-1 | done (first cut) | Firmware: tasks, settings in NOR, developer channel, watchdog task fed only on RT + main + UI progress (verified: a hung main executor resets in ≈ 3 s), test safety window |
| PWR-2 governor | superseded by PWR-5 (2026-11-26) | `orpheus_power::governor` (host-tested): Power mode as the owner defined it, 24–288 MHz |
| PWR-5 (D-4) | Max level verified on the player (2026-11-24); **D-4 revision built 2026-11-26 (host + firmware build), not yet run on the player**: Settings › System › Performance removed (key `0x0C07` retired), the governor reaches 288 MHz by itself, Power mode as the owner defined it; battery logger and `xtask drain report` built (not yet run) | Lead's checks in the session entry "PWR-5: Performance removed …"; U-PWR-5 (USB at 24 MHz), U-PWR-6 (power per level, M13); user check 19 (drain per Power mode, unplugged) |
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
| OTP-1, BT-1 | done (host) | `orpheus-otp`, `orpheus-bt` |
| USB-6 | **built (host + firmware), not yet run on the player** | MMC/CD-ROM and the extent mapper (host, earlier); export rules and `map_file` (`orpheus_usb::msc::image`); firmware `crate::drive` + image personality (`usb image <path> [cd\|disk\|fixed] [ro\|rw]`, `usb image status`); card hand-over as the card reader; read-only always for CD, writable only for `/.orpheus/` images in test mode; `xtask dev usbimage-test` (put → present → read the block device → SHA-256 → detach; `--rw`, `--qemu`). User check 26 |
| UI-6 (USB Drive) | **built (host + firmware), not yet run on the player** | USB Drive app (Images / Test images / Choose file…, options, status with counters, Back disconnects, refusal reasons), Files and the USB dialog open it; simulator back end with a fake computer; 94 goldens; no app is "not available yet" any more |
| USB-5 | **built, not yet run on the player** | Keyboard personality (developer channel + HID boot keyboard, 1 interrupt IN, 8-byte reports, bInterval 4), `orpheus-hid` (descriptor, reports, US/UK/DE/FR layouts, paced typist, fake host; for BT-4 too), `kbd on/off/type/status/layout/keypad/delay`, Settings › USB › Keyboard layout / Digits from keypad / Enter after code, udev rule (owner: reinstall). U-USB-11 open |
| OTP-2 | **on the player** (suite `authenticator`, PIN pad, passes 2026-11-27); unlock, codes and typing not yet checked by the owner | Authenticator app (PIN pad, list with codes and countdown, big code, type over USB, import from `/.orpheus/otp-import.txt`, PROVISION 0x0B + `xtask dev otp-add`, delete/move/next code/change PIN/wipe), key-only vault access in `orpheus-otp`, 190 goldens. Not built: manual entry/`TextEntry`, rename, passphrase. User check 24 |
| BT-2 (device) | **built, not yet run on the player** | UART1 driver (`atj2157_hal::uart`, `yoton_bsp::bt`), `orpheus_bt::driver`, firmware `bt` task, `BluetoothService`, shutdown 0x24 on power-off. Module off at boot; `bt probe/on/off/status/scan/connect/listen/shutdown/rx/tx` test commands. Not done: volume policy, metadata at track change, pairing UI, U-BT-2 |
| SENS-1 | **on the player** (suite `steps` passes 2026-11-27); step counting not yet checked by walking | `orpheus-steps` (detector, history, trace format; synthetic-trace tests), `yoton_bsp::accel` (no FIFO: part unidentified, U-ACC-1), `firmware::sensor` task (25 Hz, SRAM3 state), `/.orpheus/steps.bin`, `acc read/regs/trace`, `steps status`. User check 21 |
| UI-5c | **on the player** (suite `steps` passes 2026-11-27) | Steps app (Today/Week, goal, distance/kcal, settings, reset), 73 goldens (UI-5d: 98) |
| UI-5d | **on the player** (suite `tools`: stopwatch, timer, alarm screens pass 2026-11-27) | Stopwatch/Timer/Alarm in the background, alert screen, beep, backlight flash, status-bar bell/dot, RTC alarm armed; wake from off waits for PWR-3c (`ALARM_WAKE_FROM_OFF = false`). User check 22 |
| UI-6a | **on the player 2026-11-24** (connect, reconnect, audio, volume — policy changed, see session entry) | Bluetooth app (status, Pair new device, Paired devices, connect progress/failure, forget, reconnect switch), `BluetoothService` lists/requests, volume policy of 06 §4.2 (0x26 on connect and change, DAC reference −3 dB while the sink takes absolute volume), address text in written order. User check 17 |
| BRAND-1 | done (host) | `docs/brand/palette.toml` (dark/light = firmware Dark/Light with Blue; `link`, `accent` web-only; WCAG AA checked for the web and after RGB565), `cargo xtask brand [--check]` → `docs/brand/generated/` (mono/small lyre, wordmark in the firmware's bold font, favicons + ICO, apple-touch, manifest icons, 1200×630 social card, `palette.css`). Firmware theme tables unchanged; generating them, the boot logo and firmware icons are follow-ups. User: approve palette and logo variants |
| DOC-2 | done (host) | README rewrite, `docs/user/` (manual index, install/update/restore, keys, Music, Now Playing, Files, Pictures, Bluetooth, Settings, Performance, troubleshooting, FAQ; each ≤ 600 words, checked by `site --check`), screenshots `docs/shots/` from goldens (`xtask site shots`). User: read the manual once |
| DOC-3 | done (host), except the clean-container CI run | `docs/dev/getting-started.md`, `architecture.md` (SVG diagrams in `docs/dev/img/`), `how-to/` (8 guides). `#![warn(missing_docs)]` on `orpheus-codecs`, the wav/mp3/flac/vorbis/speex codecs, `orpheus-fs`, `orpheus-settings` |
| DOC-4 | done (host); workflows not run (no CI here) | CONTRIBUTING, CODE_OF_CONDUCT (Contributor Covenant 2.1), SECURITY, CHANGELOG, issue/PR templates, workflows `gate.yml`, `docs.yml`, `pages.yml` (actions pinned by SHA) |
| WEB-2 | done (host); not yet on the live site (the lead pushes) | `cargo xtask wasm [--test] [--record] [--demo]` → `target/orpheus/site/try/pkg/`: `sim/web` (`orpheus-web`) = the `orpheus-apps` screens over the simulator's services, a FAT32 card built in the browser (synthesised demo set, the visitor's files, the saved library), the firmware's codecs + DSP chain into an AudioWorklet, key recorder/replayer in the `ui_keys` corpus format. Try-it page (`site/pages/try.md`, `site/try/`): SVG player with clickable keys, integer-scaled pixelated canvas, IndexedDB state and files, Factory reset, developer panel. Self-test `tests/web/try.mjs` passes in Chromium 155 and Firefox 157 (headless); in `pages.yml`. Safari not tested: user check 23 |
| UPD-1 | **built (host + firmware), not yet run on the player** | `orpheus-update` (`.ofw` format, slot choice, power-loss-safe installer, recovery switch; power cut before every NOR operation tested), Settings › System › Update firmware (33 goldens), card and developer-channel (`UPDATE` 0x0D) paths, `xtask dev update/slots/switch-slot`, `.ofw` from `xtask build`. No RAM installer (full layout writes the inactive slot). Stage 2 fallback to the other slot (also a superseded one) after three failed starts: built in T-6, not yet installed. User check 27 |
| WEB-3 | **built (host); not yet run against the player** | `orpheus-host` over one async `Transport` (nusb / WebUSB / mock player), flows (backup ×2, first install, restore, recovery) with sector-0 permit and resumable writes, `orpheus-install-web` (wasm), Install page with demo mode, WebUSB + MS OS 2.0 descriptors on the developer interface; `tests/web/install.mjs` Node suite + `--page` pass, `--device` HIL not run. No release published (manifest, payload licences: lead) |
| WEB-1 | done (host); not published | `cargo xtask site [--check]` → `target/orpheus/site/`; Home, Manual, Developers, Hardware, Changelog, FAQ, Try it and Install (placeholders for WEB-2/WEB-3). Deploy only with repository variable `PAGES_ENABLED=true`. User: review the site |
| AU-13 | **built; first player run 2026-11-24: capture runs, microphone silent → front-end fix + `rec raw`/`rec width`/`rec mic` (not yet re-run); suite `recorder` screens pass on the player 2026-11-27** | `orpheus-rec` (WAV PCM, IMA ADPCM, FLAC 0–2 encoders; low-cut, AGC, meter, voice activation; names; write-behind ring; the recording session), `atj2157_hal::adc` (vendor capture sequence VERBATIM, DMA ch 6), `firmware::rec` (recording mode of the card task, SRAM3 state after the radio's), developer `rec start/stop/status`. Not built: voice (Opus/Speex) encoders, mic monitoring, pre-record. User check 25 |
| UI-5b | **built (host + firmware), not yet run on the player** | Recorder app (meters, timer, pause/mark/stop question, gain, menu with source/format/quality/AGC/VOX/split/monitor/low-cut), Recordings… (newest first, play, delete with question); simulator back end over the same session and a fake ADC; 122 goldens, 10 behaviour tests |
| UI-8 | **done (host); not yet on the player** | List roll-around in every list (`ListView` + `ListView::step_plain`), Settings › Display › List roll-around (`0x030C`, on by default), held keys stop at the end; Up next/Details keep their view change. User check 26; the on-device journey belongs to VER-7 |
| USB-W1 (bug) | **fixed and self-recovering (host-tested, built); not yet run on the player** | Root cause: the SETUP read was not cancel-safe (embassy-usb drops it on bus events; the interrupt stayed masked for good) and late stages landed on the next transfer. Drop-safe driver, liveness recovery, watchdog only when recovery cannot help, counters in `STATS` 24–30, soak logs self-recoveries, `xtask dev usb-cancel`. 06 §2.6; user check 28 |
| VER-10 (static stack) | **tool done (host-tested), gate step; firmware cut from ≥ 30,232 to 22,900 B, then to 16,188 B nested worst case of 22,528 B usable: margin 6,340 B, goal ≥ 4 KiB met (built, not yet run on the player)** | `cargo xtask stack [--check] [--root P]`, `stack.toml`, `budgets.toml stack_static_margin_bytes` = 6,084 B (the measured margin less one 256 B step: a ratchet). Second round: the card task's poll frames (the volume and the decoder no longer move by value; `idle`'s and start-up's children are futures of their own), .bss −512 B. Lead: soak, a Vorbis seek, the Authenticator, card hand-over to USB (session entry 2026-11-25 "VER-10: card task frames") |
| SYN-1 | **done (host)**; on the player since SYN-3 | `orpheus-synth`: band-limited oscillators (polyBLEP/BLAMP, sine, wavetables, white/pink noise), 2–4 operator FM (8 algorithms, feedback), ADSR (linear/exponential), pitch envelope, SVF LP/BP/HP, LFOs (Hz or tempo sync), 16 voices + 4 fade-out tails with stealing, mono legato glide, sample-accurate events, 6 drum voices, delay/chorus/reverb sends, soft limiter; 69,040 B engine; golden hashes equal at opt-level 1 and release; player estimate: 16 saw-lead voices ≈ 79 % of 144 MHz (session entry 2026-11-25 "SYN-1") |
| SYN-2 | **done (host)**; on the player since SYN-3 (samples and the delay compiled out there); **user check: listen to the kits and effects** | `orpheus-synth`: 11 drum models (new rim, cowbell, cymbal, clave, shaker), kits as data (9-byte entries, choke groups, 5 factory kits incl. a 32-note GM kit), sample playback (block `SampleSource`, bounded page cache with pinned heads and streaming, WAV 8/16/24/32/float, Hermite interpolation, loops), delay to 1.02 s (exact at tempo, low cut), 8-line FDN reverb at half rate in 8 KiB, `OSB1` sound banks; engine 62,320 B (was 69,040); player estimate: drum voices ≈ 75–315 cycles/sample, reverb ≈ 290 (session entry 2026-11-25 "SYN-2") |
| TTS-1 | **done (host)**; not in the firmware, simulator or apps (TTS-2, A11Y-1, BOOK-3) | `orpheus-tts`: incremental converter (1,408 B, any feed sizes), English normaliser, NRL letter-to-sound rules + stress heuristic + stems, CMUdict exception lexicon (4,487 entries in 32,766 B; `cargo xtask tts-lexicon`), duration model for 0.25×–10×; rules alone 79.8 % of tokens, with the lexicon 98.1 % (session entry 2026-11-25 "TTS-1") |
| TTS-2 | **done (host)**; not in the firmware, simulator or apps (A11Y-1, BOOK-3, firmware integration) | `orpheus-tts`: cascade/parallel formant synthesiser after Klatt 1980 (KLGLOTT88-like source with polyBLEP, F1–F6 cascade, nasal pole/zero, parallel frication/bursts), phoneme targets and loci from the phonetics literature, intonation (declination, accents, 5 contours, asides), voices Neutral/Low/Bright, exact TTS-1 durations at 0.25×–10×, resampler to 8–48 kHz, `Synth` (1,936 B) and `Speaker` (3,352 B with the converter), events and stop; WER (whisper base.en) 13.9 / 30.5 / 93 % at 1/2/4× (espeak-ng 5.9 / 63 / 89 %); player estimate ≈ 22 % of Base at 48 kHz (over the 20 % budget), code ≈ 70 KB without the lexicon (over 64 KiB) (session entry 2026-11-25 "TTS-2") |
| TTS-3 | **built (host + simulator + firmware feature `speech`); in the default image since PG-2 (2026-11-27); on the player: suite `screen_reader` passes (`said=Home. Music, button, 1 of 12`, ducking with music); owner's listening check open** (was: 133 of 128 code pages; user check 36 settled by PG-2) | Speech voice: `orpheus_tts::mix` (resampler, speech volume, ducking, stop fade; host-tested), the speech task (thread mode, `Speaker` at 16 kHz into a 128 ms ring), the card task mixing after the DSP chain or running the DAC at 16 kHz alone (`Book::rewind` on a stop), clock floor Mid while speaking (`governor::with_speech`), Music while speaking › Paused (`services::SpeechPause`), lexicon in three ≤ 16 KiB pieces, `ui speech`, journey `screen_reader` (skipped without the feature: journey field `needs`) (session entry 2026-11-27 "TTS-3") |
| VER-13 | **tool and matrix done (host + simulator); second round: six settings wired (host + simulator + firmware built), journeys `settings_sound`/`settings_playback` pass on the player (2026-11-27)** | `cargo xtask settings-matrix [--check] [--strict]` (alias `settings coverage`; in `test all`, `--strict` in `gate --m11`); manifest for all 162 settings of the tree; simulator matrix over 59 screen + 11 service contexts (`sim/tests/settings_matrix.rs`), host tests for sound, governor, steps, recorder split, persistence/defaults/clamping/migration per kind; journeys `settings_shuffle` (passes on the player), `settings_sound`, `settings_playback` (RAM-only session). Key click, Fade on pause, Gapless, Resume at start-up, Seek step, Auto-bookmark have their effect; 9,927 / 9,974 cells; 16 settings without an effect (Speed: time stretch not built, spec change proposed), 63 reviewed exclusions (session entries 2026-11-26 "VER-13") |
| VER-13 | **tool and matrix done (host + simulator); device journeys pass on the player (2026-11-27)** | `cargo xtask settings-matrix [--check] [--strict]` (alias `settings coverage`; in `test all`, `--strict` in `gate --m11`); manifest for all 162 settings of the tree; simulator matrix over 59 screen + 10 service contexts (`sim/tests/settings_matrix.rs`), host tests for sound, governor, steps, recorder split, persistence/defaults/clamping/migration per kind; journey `settings_shuffle` (RAM-only session). 9,365 / 9,412 matrix cells covered; 22 settings have no effect to show (not wired) and 47 contexts are reviewed exclusions (session entry 2026-11-26 "VER-13") |
| VER-13 (Display, Power, Date & time, USB) | **on the player** (suites `settings_screen_timers`, `settings_screen_saver` pass 2026-11-27) | Backlight timeout, Lock delay, Screen saver, Animations, Rotate (software 180°), Power mode, Keys when locked (transport to playback), Time zone, Correction, Set from RDS (group 4A), Read-only card, Developer channel (held on under any host test; the session refuses it) have their effects; journeys `settings_screen_timers`, `settings_screen_saver`; Brightness (missing stock duty values, U-PWM-3) and Wallpaper (Set as wallpaper not built) left; stack margin 4,568 B, .bss 125,352 B. User check 30 (session entry 2026-11-26 "VER-13: Display, Power, Date & time and USB") |
| GAME-6 Bricks (M14) | **built (host + simulator + firmware), not yet run on the player** | Brick breaker of our own: integer physics with one-pixel sub-steps (no tunnelling at any speed, no floating point), paddle angles, 1–3-hit and unbreakable bricks, 8 hand-made + generated levels, five power-ups, lives, scores, saves; 7 settings packed into 1 word; 47 goldens; fuzz `bricks_keys`. In the firmware (`orpheus-apps/bricks`): stack margin 4,116 B unchanged. Lead: session entry 2026-11-25 "GAME-6" |
| BOOK-1/2 wiring | **on by default since 2026-11-27, on the player** (`fa301aaf`: 127/128 code + 61/64 rodata pages, stack margin 4,276 B; suite 26/26 with `books`). **Open: opening a book while MP3 plays gives ≈ 2 underruns (`job:bread` pieces of 49–256 ms), one "The book could not be opened" (intermittent); idle Home 193 loads/s (was 138)** (session entry 2026-11-27 "BOOK-1/2: Books on by default") | `Shelf` (screens' model) / `Desk` (card side) split, step future 5,464 → 2,600 B; chores `job:bwrite…blay` while playing (no folder made, no scratch), steps while paused, idle with the arena's first 35,072 B lent; "Stop the music to prepare this book."; `books status`, `books test on\|off`; suite journey `books` (`requires`: skipped without the feature) (session entry 2026-11-26 "BOOK-1/2 wiring") |
| BOOK-2 | **done (host + simulator)**; not in the firmware (as BOOK-1; the scratch is the decoder arena, see session entry) | EPUB 2/3 in `orpheus-books`: zip (end record, streamed central directory, CRC, zip64/encrypted/method refusals), `no_std` inflate (48 B state; 32 KiB window + 2,304 B tables lent: 35,072 B), container/OPF/nav/NCX, XHTML reduction over the XML tokenizer split out of FB2, contents positions from anchors, all in bounded steps into BOOK-1's stream; library titles from the package; 12 new goldens; fuzz targets `books_zip`, `books_epub` (session entry 2026-11-25 "BOOK-2") |
| MIDI-1 | **done (host + simulator)**; on the player since SYN-3; **user check: listen to the three MIDI renders** | `orpheus-midi`: Standard MIDI File parser (formats 0/1/2, running status, meta and SysEx, SMPTE, RMID) streamed through 64 track cursors of 64-byte windows that ask for bytes (`Need`/`supply`), nothing trusted; exact tick→frame clock through tempo changes; sequencer (CC 1/6/7/10/11/38/64/91/93/96–101/120–127, RPN bend range, pressure, programs, kits on channel 10, mute/solo); own GM set of 32 sounds for the 128 programs (`data/gm-sounds.osb`); `MidiPlayer`: open/scan, render, bounded-step seek (bit-exact where nothing sounds across the target), duration/position, `.kar`/lyric events; Files plays `.mid`/`.kar` in the simulator with Now Playing lyrics; player 70,224 B; estimate ≈ 117 MHz at 48 kHz for a 10–11-voice song: High clock (session entry 2026-11-25 "MIDI-1") |
| SYN-3 | **done (firmware build)**; bench run on the player 2026-11-26 (costs measured); load governor run on the player (`78b316b5`: no reset, but every song stopped at `ratio=400`); measurement fix and clock-up (D-4) on the player (`8240f128`: ratio right, 288 MHz, but paging 200–250 %); paging fix on the player (`4e1e9a49`: every song to its end, `pg=` 3–15 %); shed-state, rest and key-tick fixes on the player (`7f26ae50`: all to the end, 20 underruns in 107 s at `load=` 25–40 %); rests that end when thread mode sleeps on the player (`b694ac61`: underruns 18/3/9 per song at `load=` 24–46 %); **budget that follows the PCM ring, main loop off the synth's lines, USB supervisor without its 20 ms timer built, not yet run on the player: lead's checks in the session entry 2026-11-26 "SYN-3: thread mode's line loads"**; owner decision: half rate for heavy MIDI files (04 §9) | The synthesiser on the player: a synth source beside the codecs (`orpheus_audio::synth`), the engine and the MIDI player built in place in the pool (stream ring + decoder arena, 80,816 B; MIDI state ≈ 71,960 B), the MIDI file through a 13-block card cache (`orpheus_midi::cache`), 44.1 kHz into the DAC ring through the DSP chain, clock floor High raised to Turbo/Max by the load governor before anything is shed (`orpheus_synth::load`: 55 % of real time, down to 30 % while the PCM ring runs short; then chorus, delay, reverb, voices shed at Max; compute measured apart from paging, warm-up 250 ms, rise ≤ 3 % a block), 4 ms pieces and pace-rule rests (thread mode always runs; a rest ends once thread mode sleeps); underruns never end a song (only "nothing can play"); `.mid`/`.kar` play from Files and Music with seek and pause; `synth bench` (36 loads, cycles, host hashes); 125/128 code pages, stack margin 5,212 B (session entries 2026-11-26 "SYN-3", "SYN-3: load governor" and "SYN-3: governor ratio measured right") |
| SEQ-1 | **done (host + simulator)**; not in the firmware (SYN-3); **user check: listen to the demo beat** | `orpheus-seq`: project model (16 patterns × 1–64 steps × 8 drum tracks; velocity, chance, ratchet, delay, accent per step; mute/solo/level; songs; 40–240 BPM, swing 50–75 %), exact integer step timing (any tempo and swing, an hour without drift), edits heard at the next step, render without UI (`Studio::render_i16`), `OSQ1` files with CRC under `/.orpheus/seq/`; Sequencer app (feature `orpheus-apps/seq`): list, grid, transport, step, track, mixer; 38 goldens, 8 journeys, fuzz `seq_load`; project 25,092 B (session entry 2026-11-26 "SEQ-1") |
| SEQ-2 | **done (host + simulator)**; not in the firmware (SYN-3); MIDI clock out built as a host model (2026-11-27), its USB transport and setting deferred to M16; **user check: listen to "Tram Lights"** | `orpheus-seq`: 2 synth tracks (note, gate, chord in the scale; transpose; scale lock), per-step parameter locks (24 a pattern, refused when full), kit B, fill while held, metronome, count-in, step length, live record quantised; `OSQ1` v2 with SEQ-1 migration; WAV export (offline) and MIDI export (exact ticks); Sound, Keys, Live and Song screens, Sequencer settings (7, word `0x1180`); project 35,808 B; MIDI clock model (Start/24 PPQN/Stop on exact frames, phase kept across tempo and step-length changes, USB MIDI packets; player 4,056 B) (session entries 2026-11-26 "SEQ-2", 2026-11-27 "SEQ-2: MIDI clock") |
| A11Y-2 | **on the player** (suite `settings_accessibility` passes 2026-11-27); journey `settings_accessibility` and user check 31 | Settings › Accessibility (Text size Normal/Large/Extra large, Hold delay 0.5–2 s, Key repeat Normal/Slow/Slower; packed in `0x0D60`), Themes White on black / Black on white (≥ 7:1), reduced motion = Animations off (spinner still, blinks lit), Key click Presses and holds (hold confirmation); 63 `a11y_*` goldens (session entry 2026-11-26 "A11Y-2") |
| A11Y-1 | **done (host + simulator)**; in the firmware with TTS-3 (feature `speech`, **on by default since PG-2**, suite `screen_reader` passes on the player; chords approved by the owner); **user check: listen to the spoken journeys** | Screen reader: `orpheus_ui::a11y` (descriptions, `Reader`, `SpeechService`, `UtteranceQueue`, earcons, chords) behind `screen-reader`; every one of the apps' 72 screens describes itself (lint: 629 foci, every screen reached); Settings › Accessibility: Screen reader, Speech rate 0.25×–10× in 0.25× steps, Voice, Pitch, Verbosity, Sound cues, Where am I, Keys with screen off, Speech volume and Music while speaking (TTS-3); toggle = hold Back + Menu 2 s (Vol+ + Vol− share a ladder); 8 spoken-journey goldens with audio hashes; matrix contexts `speech`, `speech-music` (session entries 2026-11-26 "A11Y-1", 2026-11-27 "TTS-3") |
| VER-13 Brightness (U-PWM-3) | **on the player** (suite `settings_brightness` passes 2026-11-27); journey `settings_brightness` and user check 32 | Settings › Display › Brightness = the stock levels 1–11 (default 6) on the stock PWM4 duties, live; key `0x030D` (old `0x0301` retired); alert flash 15 / 3 (never 16); `ui top` `bl=` |
| VER-13 (DOC-2 findings) | **done (host + simulator + firmware built); on the player: not yet** (2026-11-27) | Time check shows UTC (`regress_auth_time_check_shows_utc`); Recording › Source Line-in retired; Output hidden (no route, U-BT-2); Settings › Storage: Clear caches, Check card (read-only), Format card (owner allowed, 2026-11-27, via the lead) work through `orpheus-fs` (`format`, `Volume::check`, `remove_tree`); System › Diagnostics read-only; journeys `storage`, `storage_clear` (not run); rodata window 64/64; stack margin 4,276 B (session entry 2026-11-27 "VER-13: the DOC-2 findings fixed") |
| VER-3 | **done (host)** | Adversarial-input suites in 27 crates (`tests/adversarial.rs`, shared helper `tests/adversarial.rs`; 99 tests, 24 `proptest` properties with fixed seeds, failures recorded in `tests/regress/proptest/`); 3 bugs fixed (F-GAME-1, F-SET-3, F-SET-4); inventory and what is not covered: `docs/verification/baseline-2026-11.md` §9 (session entry 2026-11-27 "VER-3") |
| VER-4 (settings store keys) | **done, on the player** (store mounts on every boot since the 2026-11-26 read-race fix; owner's settings intact) | Store index 128 → 192 keys at 4 B a slot (was 8), same NOR format both ways (fixture from the old code, byte-equal images); cache write marks unlimited; 131 keys in use with every app (61 free), firmware 129; sector ceiling ≈ 209 settings keys (256 cannot fit one sector); .bss −1,248 B, stack margin 5,664 B; compaction no longer re-blesses a rotted value (session entry 2026-11-26 "VER-4: settings store index") |

## 3. Session log

### 2026-11-27 — VER-13: the DOC-2 findings fixed; Storage and Diagnostics on the player (worker, no hardware)

Host, simulator and firmware build only (no `xtask dev`: another worker
had the player). The six findings of the DOC-2 entry below, plus the
lead's update during the session: the owner replaced the player's card
with one that is safe to format and allowed Format card (2026-11-27,
relayed by the lead; not heard from the owner by this worker), so it is
built rather than hidden and there is no §5 question about it.

* **Authenticator › Time check** showed the clock's local time labelled
  "UTC" (`screens/auth.rs` drew `now()`). It shows the UTC the codes are
  computed from (`unix_time()`, the clock less Time zone).
  `regress_auth_time_check_shows_utc` (sim/tests/auth.rs; fails without
  the fix: 12:41 "UTC" at +01:00); golden `auth_time_check_tz_dark_latin`
  (status bar 12:41, dialog 11:41:00 UTC).
* **Recording › Source** offered Line-in; the YM03 has no line input (01
  §9 row "Audio inputs on the board", inferred). Value 2 retired: two
  choices, a stored 2 reads as Microphone (`validate`), `rec_source(2)` is
  Microphone; `RecorderService::rec_has_line_in` and the simulator's
  `set:recline` removed; `rec start line` (developer) stays.
  `regress_rec_source_line_in_retired`; effects.toml 0x0801 values
  Microphone, FM radio.
* **Output (0x0604)** was never read: there is no route to choose (the
  module takes its audio after the amplifier, U-BT-2: jack and sink carry
  the same sound, neither can be muted alone). Hidden (`settings::hidden`,
  as Speed): not in Settings › Bluetooth nor the quick panel; the dead
  Now Playing handler removed; key kept. effects.toml: `contexts = []`
  and a `gap`. `regress_output_setting_offered_without_effect`
  (sim/tests/settings_matrix.rs: neither list offers it, no frame
  changes). Quick-panel goldens (6) and `app_group05` re-blessed.
* **Storage and Diagnostics**, built (05 §7.11 "As built (VER-13 …)"):
  * `orpheus-fs`: `format` module (FAT32 to 32 GiB, exFAT above and below
    33 MiB; on the volume's own region: the MBR's first FAT/exFAT
    partition or the whole superfloppy; the partition table never
    written; GPT and unknown layouts refused, nothing written; boot
    sectors zeroed first and written last, so a cut format does not
    mount and formats again; label ORPHEUS; the exFAT up-case table is
    mkfs.exfat's byte for byte, from four identity runs and `upcase16`,
    so a formatted card keeps the fast up-case path); `Volume::check`
    (reads every folder to a cursor-stack depth, follows every file's
    chain to its size, writes nothing, stops when asked);
    `Volume::remove_tree` (inside the write prefix); `region_of` shared
    by mount and format.
  * `orpheus-apps`: `StorageOutcome`, `CACHE_DIRS` (`/.orpheus/cache`,
    `/.orpheus/index`), `CHECK_DEPTH` 8, `other::STORAGE`, `Diagnostics`;
    Settings › Storage: one card job at a time ("The card is busy"),
    "Started", then a toast (Done / Card formatted) or a dialog (No
    problems found · Problems found + "Check the card on a computer." ·
    Check stopped · The card could not be read · Format failed). Format's
    confirmation (destructive, Cancel focused): "All music and files on
    the card are erased." Diagnostics: a read-only page (Uptime,
    Processor MHz, Code loads, Audio underruns, Free stack).
  * Firmware: `care.rs`; `media::Req::Care` → the idle loop hands the
    card over as for USB (`hand_over`: playback stopped, state saved),
    `audio::release` runs `care::run` (Clear, Check) or `care::format`
    instead of the USB hand-over, then `card_back`. Format: the volume
    flushed, the card formatted through it, the old volume dropped
    without a flush, mounted again. Check stops when any request for the
    card task arrives. `care:` log line. `Fw::care_poll` raises
    `other::STORAGE`; Diagnostics from `uptime`, `delay::core_hz`, pager
    loads, `audio::UNDERRUNS`, `rt::stack_unused`.
  * Simulator: the same `orpheus-fs` calls on its card (`Media::
    format_card`, `clear_caches`, `check_card`), `set:cardbad=1` (a file
    with a broken chain).
  * Tests: `crates/orpheus-fs/tests/format.rs` (7: FAT32 and exFAT on an
    MBR partition, `fsck.fat`/`fsck.exfat -n` clean before and after use,
    the MBR and gap untouched, exFAT by size on a 33 GiB sparse image,
    label, superfloppy, refusals, a format cut at five points);
    `tests/care.rs` (4: counts, read-only, depth, stop, a broken chain,
    remove_tree only the tree and only under the prefix, `fsck` clean);
    unit `upcase_table_is_the_standard_one`, `layouts`;
    `sim/tests/storage.rs` (6, with `regress_diagnostics_was_not_available`:
    it opened "Not available yet"). Goldens `app_check_card`,
    `app_check_card_problems`, `app_formatted`, `app_diagnostics` (6
    each), `app_format_dialog` (new text) — looked at. Suite: journeys
    `storage`, `storage_clear` with expected frames (looked at; counts and
    values masked); `home/quick` and `settings/bluetooth` re-blessed (no
    Output row). The simulated player's `ls` (xtask `SimPlayer`) answered
    a fixed root listing for any path; it reads the simulated card now
    (`Media::ls`), so `storage_clear`'s `ls` checks run in the dry run.
* **Stale text**: `orpheus-apps/Cargo.toml` (games, Salvo, Bricks, Books,
  MIDI, screen reader are in the firmware; the Sequencer is not, and why),
  `strings.rs`, `orpheus-ui/src/a11y.rs`; 05 §7.13 (games feature),
  §7.14, §7.15, §7.16 (Sequencer not wired, not "until SYN-3").
* **Radio spec** (05 §7.3) now says what the code does: Down next preset,
  Up previous (as every list; the user page already said so), steps and
  seeks wrap at the region's limits, Wide 65–108 MHz, RDS clock built
  (Set from RDS). No code change.
* **Game sound effects**: 05 §7.13 marks them not built.
* **Docs**: `docs/user/settings.md` (Storage section, Diagnostics, no
  Output), `authenticator.md` (Time check is UTC), `keys.md` (quick
  panel); `recorder.md` already said Microphone and FM only.
* **Firmware numbers** (`xtask build`, speech + Books): resident 60,556 →
  60,612 B; `.bss` 147,352 → 147,360 B; code 127/128 (8,194 B free,
  unchanged); **rodata window 61 → 64 of 64** (cold code 21 → 22 pages,
  164,536 → 176,410 B; the formatter is 5.3 KB, Check and Clear about
  1.2 KB each): the code window's reserve page (8 KiB) is now the only
  room left for code; slot 1,605,632 → 1,630,208 B. `stack --check`:
  13,644 B of 17,920 B, **margin 4,276 B (unchanged)**, the worst chain
  the Vorbis set-up as before. What it took: each card job polled in its
  own frame (`Own`/`Apart`), `release` a two-arm dispatcher (`to_usb` the
  old body), Care folded into `Idle::Release` (an arm of its own grew
  the task's poll in the `play-pcm` cluster by 196 B and made `play-fill`
  share lines with `mp3-frame`/`flac-frame`), the formatter's two
  512-byte locals moved into the caller's buffer, `check`'s stop callback
  generic (an unresolved indirect call), and Format holding no `Card` in
  its future (the card task's future was +312 B in `.bss`: over budget
  and one 256 B stack step lower). `check-resident` ok.
* **Gate**: `cargo fmt --all --check` (root, `fw/`); `cargo clippy
  --workspace --all-targets -D warnings`; `fw/` clippy as the gate runs
  it; `cargo test --workspace --no-fail-fast` 2,284 passed, 0 failed, 14
  ignored; `settings-matrix --check` 184 settings, 13,171 / 13,217 cells
  (99.7 %), 4 without an effect, 50 exclusions, 0 errors; goldens (in the
  workspace tests); `xtask build`, `check-resident`, `size --check`,
  `stack --check`, `xtask test pagelink` (self-test passed); `brand
  --check`; `site --check` (134 files; `site shots` rewrote
  `app_quick_dark_latin`; settings.md cut back under 600 words). **Not
  run**: every `xtask dev` step (the brief).
* **On the player, for the lead** (not run here):
  1. `cargo xtask dev suite storage` (new journey): Check card on the
     owner's card, the result dialog, Diagnostics. The 60 s wait is a
     guess: read `care: check ok dirs=… files=… problems=…` in the log for
     the time it took and the counts; adjust the wait. Then
     `--update-expected` for its shots after looking.
  2. `cargo xtask dev suite storage_clear` (new, last in the run order):
     Clear caches; `ls` shows both folders gone; browse a large folder
     after (index rebuilt).
  3. Format card (**lead/owner check, no journey**; user check 37 in
     USER-CHECKS.md covers 1–3 by hand): on the card the owner
     said is safe to format only. Settings › Storage › Format card…,
     Format; "Card formatted"; Card info shows FAT32 (≤ 32 GB) or exFAT;
     Files empty; the card then reads on a computer (label ORPHEUS,
     `fsck` clean) and takes files; settings and the vault unchanged.
  4. Authenticator › Time check with Time zone ≠ 0: the time shown is UTC.
  5. Settings › Recording › Source: two choices; Settings › Bluetooth and
     the quick panel: no Output row.
* **Open**: Check card's duration on a large card and whether to run it
  while music plays (now it stops playback, as Clear and Format); the
  spec's full Diagnostics (key, display, audio, sensor tests, card speed,
  event list) is not built; the rodata window is full.

### 2026-11-27 — DOC-2 refresh: docs and site brought up to date (worker, no hardware)

Owner's request: "ensure the web page and documentation is up to date and
reflects current status". Each claim checked against §2, the session
entries, USER-CHECKS.md and the code; three states used throughout:
works (used on the player), on the player but being checked (suite
passes there, a person's check open), simulator only / planned.

* **Changed**: README (status 2026-11-27, MIDI, new shots), CHANGELOG
  [Unreleased] (Added/Changed/Fixed/Known issues to 2026-11-27),
  SECURITY.md (Authenticator built; fixed device label, no passphrase),
  `docs/dev/architecture.md` (cold code in the rodata window, clock
  governor and Power modes, card task and speech), `docs/user/` README,
  settings, power, install (update paths and their state; browser
  installer untried on a real player), keys (chords; power menu without
  Start original firmware), music (MIDI), now-playing, files, faq,
  troubleshooting, usb-drive; site `index.html`, `install.md`, `try.md`,
  `hardware.md`.
* **New manual pages** (each ≤ 600 words): radio, recorder, steps,
  stopwatch, authenticator, games, salvo, bricks, books, books-reading,
  accessibility, screen-reader, sequencer; 39 new shots in `docs/shots/`.
* **Gate**: `cargo xtask site --check` (134 files, Home 78 KB) and `site
  shots --check` pass; `cargo test -p xtask` passes; no code changed, so
  the workspace tests were not rerun.
* **Found (not fixed; for the lead)**: §2 rows stale against later
  entries — T-6 and §1's NOR row ("fallback not installed"; installed and
  crash-tested 2026-11-24), AU-13/UI-5b, OTP-2, SENS-1, UI-5c/5d, A11Y-1/2,
  TTS-3 ("not yet run on the player"; the suite runs them since PG-2), and
  user-check numbers in rows 48/50/52/62 differ from USER-CHECKS.md.
  Firmware: Storage › Clear caches / Check card / Format card and System ›
  Diagnostics do nothing on the player (only RebuildLibrary is wired,
  `fw/firmware/src/ui.rs`); Settings › Recording › Source offers Line-in
  though the board has none; Authenticator › Time check labels local
  time "UTC" (`screens/auth.rs`); the Sequencer is hidden from Home in
  the simulator and the Try it page (`set:seq=on`); game sound effects
  (05 §7.13) are not built; stale "not in the firmware" comments in
  `orpheus-apps/Cargo.toml`, `strings.rs` and `orpheus-ui/src/a11y.rs`.

### 2026-11-27 — SEQ-2: MIDI clock out as a host model; the rest of SEQ-2 was already merged (worker, no hardware)

Host only (no `xtask dev`: the brief). The brief asked for all of SEQ-2;
everything but the USB MIDI clock was built and merged on 2026-11-26
(`538b9128`, `c3041aeb`; entry "SEQ-2" below: synth tracks, note entry,
scale lock, locks, Sound/Keys/Live/Song screens, live record, WAV and
MIDI export, 7 settings, 40 goldens), and its gate passes again here. What
was left is the clock, built now as the host-side model; full "as built"
in 05 §7.16 ("As built (SEQ-2, 2026-11-27): MIDI clock out …").

* **Model** (`orpheus_seq::clock`, `Sequencer::set_midi_clock` /
  `take_clock` / `clock_dropped`): Start on the first step's first frame
  (after the count-in, which sends nothing), 24 Clocks a quarter from the
  same frame, Stop on the frame after a stop, a restart or turning it off;
  clocks in the player's own units through its units → frames map, so
  clock *k* is on ⌊*k* · rate · 250 / tempo⌋ at any step length and swing;
  tempo and step-length changes keep the phase. 32-message queue (8 B
  each), drops counted, phase kept. USB MIDI 1.0 event packets
  (`Realtime::usb_packet`). No SPP or Continue (Orpheus always starts at
  position 0).
* **Deferred to M16**: the USB MIDI interface that carries it, timing the
  sends to the DAC, and the setting "MIDI clock out" (no setting added:
  there is nothing for it to switch yet, so no `effects.toml` entry).
* **Numbers**: player (`Sequencer`) 3,784 → 4,056 B (`ram_table`'s 4 KiB
  limit holds); studio 40,424 B; WAV export 4,080 B. Firmware unchanged
  (`orpheus-seq` is not in it): resident 60,332 B, 127 code + 46 rodata
  pages, bss 134,368 B, stack margin 16,412 B.
* **Tests**: `crates/orpheus-seq/tests/clock.rs`, 9: exact clocks at 8/
  44.1/48 kHz × 40–240 BPM × swing 50/57/75 × three step lengths with an
  odd pattern length; steps on clocks (every 6/12/3, with swing every
  pair); tempo change and step-length change keep the phase (the change's
  step on a clock); count-in silent; stop/restart/off/on-while-playing;
  any block size the same (the demo song in song mode); full queue; USB
  packets.
* **Gate**: `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -D warnings`; `cargo test --workspace` 2,160 passed, 0
  failed, 13 ignored (goldens included, none changed: no new screen);
  `settings-matrix --check` 184 settings, 13,248 / 13,295 cells, 0
  errors; `xtask build`, `check-resident` (none paged), `size --check`,
  `stack --check`: pass. **Not run**: `brand`/`site --check`, fuzz,
  every `xtask dev` step (the brief).
* **User check** (unchanged from SEQ-2): listen to "Tram Lights"; with
  M16, a DAW following the Sequencer's clock.
* **Cleanup**: the SEQ-1 and SEQ-2 package rows had landed in the session
  log by merges; moved into §2's table.

### 2026-11-27 — VER-3: adversarial inputs (worker, no hardware)

* **Suites**: a `tests/adversarial.rs` in every crate that reads outside
  data — `orpheus-update` (`.ofw`, `UPDATE` requests in any order with
  power cuts), `orpheus-usb` (every SCSI opcode × CDB length × fill × unit,
  LBA/count/allocation edges, CBW cut/damaged, devchan every request),
  `orpheus-fs` (cyclic/broken FAT chains, exFAT root loop, boot-sector
  edges, hostile names + fsck, write-prefix escapes, locator edges), the
  codecs (every seed cut in its headers, arena one byte short and reused,
  WAV/FLAC/Ogg/ID3 field edges, tag readers), `orpheus-img`,
  `orpheus-books` (zip field edges, CD lies, hostile paths, markup soup),
  `orpheus-midi`, `orpheus-synth`, `orpheus-seq`, `orpheus-games` (saves
  with a right CRC), `orpheus-steps`, `orpheus-apps` (settings records,
  stepping), `orpheus-fm` (RDS, presets), `orpheus-hid`, `orpheus-bt`
  (events × lengths × checksums × chunking; requests and events in any
  order), `orpheus-pager` (traces in any order with shrinking), the
  simulator (a hostile language pack on every screen), and seed sweeps
  through the fuzz bodies for otp, proto, settings, index, library,
  player, sdcard, input, power, tts. Shared helper `tests/adversarial.rs`:
  `prefixes`, `damaged`, `edges`, `hostile_names`, `seeds`, `sweep`,
  `config` (proptest: fixed seed per property, ChaCha, failures recorded
  in `tests/regress/proptest/<name>.txt`). `proptest` 1.12 is a workspace
  dev-dependency only (default features off: no fork, no timeout).
* **Counts**: 99 tests in 27 files, 24 of them `proptest` properties
  (64–512 cases each), 1 `#[ignore]`d (every byte of every codec seed:
  `cargo test -p orpheus-codec-conformance --test adversarial --
  --ignored`; run once: **passed, 2,309 s**, every byte of all 115 seeds,
  no finding). Each suite alone
  ≤ 6 s; `cargo test --workspace` 196 s (165 s before, both including the
  incremental build).
* **Bugs found and fixed** (each with `regress_*`, input under
  `tests/regress/`, a row in `docs/regressions.md`, checked failing
  without the fix):
  * F-GAME-1 `Bricks::check` (run by `decode` on every save from the
    card) overflowed on a save with a right CRC and a large broken-brick
    counter or paddle/ball position — a panic in the simulator/browser, a
    wrapped score sum in the firmware that could load an unreachable
    state. `regress_bricks_save_check_overflow`,
    `tests/regress/games_save/`.
  * F-SET-3 `Setting::step` overflowed `i16` at the ends of the packed
    words' full-`i16` ranges (0x0D60, 0x0D61). `regress_setting_step_overflow`,
    `tests/regress/apps_settings/step-at-i16-limits`.
  * F-SET-4 `format_value` of the Speech rate (`Unit::Quarters`) overflowed
    `u16` beyond ±2,621. `regress_speech_rate_format_overflow`,
    `tests/regress/apps_settings/format-quarters-out-of-range`.
* **Oracle corrections** (by design): Vorbis refuses a short arena at its
  first `decode` (set-up header) with `ArenaTooSmall`; PNG/JPEG check
  their need with slack, so the exact used size may be refused with
  `Arena`; a decoded game save is not the identity (Solitaire's cards in
  hand, Salvo's shot in flight); a repeated update piece with other bytes
  fails the update.
* **Not covered yet** (baseline §9.3): `orpheus-audio` ring with arbitrary
  read sizes, `orpheus-dsp` float extremes, `orpheus-ui`, `orpheus-rec`,
  the host tools against garbled device responses, `xtask` inputs,
  firmware-only concurrency (VER-7/8).
* **Firmware** (`orpheus-games` and `orpheus-apps` changed): `xtask build`
  ok — resident 60,336 B, **127/128 code pages (8,362 B free)**, 46/64
  rodata, slot 1,482,752 B; `stack --check` 14,820 B of 31,232 B (margin
  16,412 B); `size --check` ok (bss 134,368 B); `check-resident` ok.
* Gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, `cargo xtask settings-matrix
  --check` (99.6 %, 0 errors) pass; `xtask build`, `stack --check`, `size
  --check`, `check-resident` pass; device steps not run (host-only
  package).
* No user checks.

### 2026-11-27 — BOOK-1/2: Books on by default (worker, player)

Brief: turn the firmware feature `books` on by default (with `speech`),
which PG-2 left off because the static stack margin was 3,460 B against
4,096 B, and make the Home suite journeys work with Books. Player: `fa301aaf`
installed with `dev update` into slot A (41.6 s, "running … from slot A";
`dev backup-verify` ok first). No flash layout change (the coordinator's
2026-11-27 permission was not needed: the image is 1,605,632 B of the
2,093,056 B slot).

* **Found first: with PG-2b the Books image no longer linked.** `cargo
  xtask build --features books` on `222b93b9`: "paged rodata and cold code
  need 33 groups of 16 KiB (20 + 13), the window holds 32". The cold
  groups were full (`used` 16,140–16,384 B each), but 32 KB of it was
  PG-2b's padding before a section moved clear of its constants' lines,
  never used again; and the code window's reserve page stayed empty.
  **pagelink**: (1) the padding is a hole later sections fill first
  (each group's sections still go to the script in address order); (2)
  when the rodata window cannot hold the rest with the reserve kept, the
  reserve gives way (`place_cold` → `place_cold_reserving`). Default image
  now: **code 127/128 (8,194 B free), rodata 61/64** (21 cold pages,
  164,214 B; 0 of 5,512 cold loads unbounceable). Two cold sections
  share a physical line with a constant they read and bounce
  (`Epub::step`, 8,124 B; the library build's `Run::sort`, 7,554 B: too
  large for the free code room); `Desk::step_with` (18 KB) stays in the
  code window (larger than a rodata group). Tests
  `pagelink::tests::regress_cold_padding_was_never_filled`,
  `regress_reserve_page_kept_while_the_rodata_window_overflowed`.
* **Stack, measured** (`xtask stack --check`, speech + Books): before
  **14,900 B nested worst case of 17,664 B usable, margin 2,764 B** (PG-2's
  3,460 B was before PG-2b's growth); after **13,644 of 17,920 B, margin
  4,276 B** (budget 4,096 B, unchanged). Thread context 5,272 → 4,504 B,
  RT (card task) 8,448 → 8,200 B, L1 deadline 312 → 72 B. The cuts, in
  the order measured:
  1. **Timer rearm** (`atj2157_hal::time`): `rearm` borrowed the queue
     with `borrow_mut`, so `panic_already_borrowed` and the panic handler
     sat under every timer wait (the bottom of the card task's deepest
     paths) and under the T1 alarm: now it takes the queue the caller
     holds (`try_borrow_mut`). L1 312 → 72, RT −40: **−280 B**.
  2. **Books' file closes** (`Desk::save_state`, the library header and
     title writes): `vol.close(f)` built an 840 B future on the stack and
     copied it (twice in `save_state`, frame 1,824 B); now
     `flush_file(&mut f)` (what `close` does). `Desk::step_with`'s worst
     path 6,368 → 3,456 B with (1) and (3); the card task's worst became
     the Vorbis set-up path (`seek_to` → `setup`): RT −48 B.
  3. **Panic handler** writes its text straight into the fault record
     (`persist::PanicText`) instead of a 96 B line buffer, and
     `LineBuf::push_u32` has no table of powers on the stack
     (`log::decimal`): the panic tail 184 → 24 B, under most deep paths:
     RT **−160 B**.
  4. **USB descriptor buffers** `CONFIG_DESC`, `BOS_DESC` 256 → 128 B
     (the largest configuration is ≤ 41 B, the BOS 64 B by `lsusb -v`;
     MS OS 2.0 keeps 256): `.bss` 147,608 → 147,352 B (budget 147,456:
     it was over) and the stack's start one 256 B step lower: usable
     **+256 B**. All four personalities' builders are unchanged.
  5. **Authenticator code** (UI task): HMAC by hand over one hasher
     (`orpheus_otp::otp::mac`, checked against RFC 4226/6238 and against
     the `hmac` crate for keys shorter, equal and longer than each
     block: `mac_matches_the_hmac_crate`): frame 1,232 → 648 B; the code
     computed from a cut-down entry (`Fw::auth_code_now` reads the entry
     and keeps only kind, algorithm, digits and the secret, wiped after):
     `auth_code_now`'s path 2,808 → 1,992 B.
  6. **PIN change** (`Fw::derive`): the sealing futures take the
     finished key (`vault::Rekeyed`, `RekeyJob::finish`) instead of the
     240 B job, and `Authenticator::finish_change`/`finish_create` run
     the derivation when called and return the small future: `derive`'s
     path 4,556 → 3,444 B.
  7. **The UI built in place** (`ui::new_ui`): the whole `Ui` (3.5 KB)
     was a temporary in its frame (2,948 B) with the screens' creation
     below; now its fields are written into the static, the shell by
     `new_shell`, which writes it in place, the rest by `ui_fields` in a
     frame of its own: path 4,456 → 3,880 B.
  Thread context after 5–7: 5,272 → 4,504 B (−768 B). The next deepest
  paths (for whoever needs more): the card task's Vorbis set-up while
  seeking (`seek_to` 1,488 + `load_setup` 1,320 + `setup` 2,576 B frames),
  `run_player`/`play_file` (552/504 B frames, under every playback path),
  the browse path's `spin_in` (1,504 B), the UI's shell creation
  (`new_shell` 2,368 B: `Shell::unentered` builds the shell as a value).
* **Books on by default**: `fw/firmware` `default = ["speech", "books"]`;
  `App::Books.on_player()` true; the simulator shows Books on Home by
  default (`set:books=off` takes it away): 52 goldens re-blessed (Home
  grids, Settings › Display › Home apps, a11y Home, radio/seq/tools Home
  shots: Settings moved to the fourth row, Books before it; looked at),
  8 spoken-journey texts ("… of 13"). Suite: `home` shoots `home_books`,
  `games` opens with `prev*3`, `books` lost its `sim: set:books=…`
  lines and turns to the first page with `prev*7` (three Prevs were not
  enough once a hand run left the text on page 4); expected frames
  regenerated (12 changed, `home_books` new; looked at). `xtask`:
  Settings is Home cell 12 for the soak (`browse_targets_on_home`), the
  player's Home-apps mask 0x1FFF. `suite --setup` sends `stop` first: a
  reboot reopens the last track paused, and an upload waited until it
  ended.
* **On the player** (`fa301aaf`, test look, cable):
  * `dev suite --setup`: **26 journeys passed, 0 failed, `books` ran**
    (library, page 1, page 2; `books status` lib=ready n=2, page=0 then
    1; EPUB left closed). Shots looked at (Home with Books, the library,
    the text's pages). Stack free 5,256 of 18,400 B (painted; `[hil]`
    minimum 2,048).
  * **MIDI folder** (`playdir /.orpheus/test/suite/midi`, `status` every
    9 s for 125 s): underruns **0/0/0** (Etude, Hill Song, Sound tour, as
    `f7b1f4c8`); line loads **≈ 1,410/s** over the folder (Etude ≈ 1,560,
    Hill Song ≈ 1,550, Sound tour ≈ 1,300) against **≈ 1,273/s** on
    `f7b1f4c8`: **+11 %**; `cpu=48`, clock Base once it ended.
  * **Idle Home** (test look, `dev stats` 10 s apart): **≈ 193 loads/s**
    (196, 193; the suite's note 180) against **138/s** on `f7b1f4c8`:
    **regression +40 %**. `dev prof --faults 15`: spread over the USB
    task and `embassy_usb`'s poll, the UI task, `Timer::poll`, the
    settings path (`target/orpheus/prof/20261127-135135`): a layout
    effect of the larger image (71 code lines faulted in 15 s); not
    fixed here.
  * **BOOK-1/2 check 2** (the text while MP3 plays, `/Knower`, `books
    test on`): opens and turns pages (page=1…4, `chore: job:bread`
    lines), but **opening it costs ≈ 2 underruns** each time (three
    opens: 2, 2, 2; turning pages 0): the log shows **`job:bread`
    pieces of 49, 70, 77 and once 256 ms** in one unsplittable stretch
    (`ub=job:bread:256ms`, DAC head −10,245 frames) against 26–30 ms
    budgets. Not found: which await takes that long (one PC profile of
    an open caught a book already open). **Once the text failed to open
    after the EPUB had been read during music ("The book could not b…",
    the message itself cut off; `book=closed`, `steps` frozen until
    `books test off`/`on`)**; the same sequence idle and once more with
    music opened it. Open findings for the lead.
  * **Check 3** (the EPUB while music plays): **not shown**: the EPUB was
    first converted idle by my step-by-step run (conv 999, lent 54,
    retaken 1, 18 pages), and re-uploading it with `suite --setup`
    kept its stamp, so it stayed cached; with music it then opened from
    the cache (2 underruns at the open, pages turned with 0). The "Stop
    the music to prepare this book." path needs a fresh EPUB (no delete
    command; another name changes the suite's library count).
  * **`dev soak music --minutes 15`: PASS, 18.3 min**, 13 actions, 113
    samples (`target/orpheus/soak/music-20261127-141003-seed1`): stack
    free min **5,256 B** (f7b1f4c8: 18,568; the Books `.bss` and the
    painted stack's size, `[hil]` ≥ 2,048), line loads/s mean **1,836**
    / max 3,949 (1,742 / 4,196), evictions/s max 2,768 (4,216),
    underruns 0, DAC headroom min −3,059 frames (inside a disruption
    window; 770 on f7b1f4c8), card errors 0, uptime vs host −22 ppm,
    **CPU idle 54 %** (62 %): mean loads +5 %, idle −8 points.
  * **Owner's settings: not written.** `settings changed` reads
    `changed: 30b=4095 701=1 d50=10470 e80=20992`: the stored values are
    as before; **`30b` is listed because its default changed**: Home
    apps (0x030B) defaults to every app (`HOME_ALL`, now 0x1FFF with
    Books, bit 12), and the owner's stored mask is 0x0FFF, saved when
    Home had twelve apps. **So the owner's own Home does not show Books**
    (screenshot with the test look off: twelve cells, Settings last); the
    suite shows it through the test look. Decision for the lead/owner:
    switch Books on in Settings › Display › Home screen by hand, or make
    a stored mask cover only the apps that existed when it was saved
    (new apps shown) — not done here (the owner's setting).
  * Ended in a reboot: the player runs `fa301aaf` from slot A (slot B
    keeps `f7b1f4c8`), Home, keys free, screen on.
* **Found, not fixed**: an abandoned upload holds the card task: `fcreate`
  queued while a reopened track was paused was served after a later
  `stop`, entered the upload loop, and that loop takes every later
  command (`stop`, `fcreate`) without answering until `fclose`
  (`audio::upload`). `dev cmd fclose` ended it (the suite's tone file was
  truncated and uploaded again). The suite now stops playback before
  uploading.
* **Gate**: host `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -- -D warnings` (also `--no-default-features`); `cargo
  test --workspace` **2,154 passed, 0 failed, 13 ignored** (203
  binaries); `cargo xtask settings-matrix --check` (184
  settings, 0 errors). fw: `cargo fmt --all --check`; `cargo clippy
  --workspace --release -- -D warnings`, also `--no-default-features`
  and `-p firmware --no-default-features --features speech`; `cargo
  xtask build` (resident 60,548 B: text 30,516, rodata 13,328, data
  16,700; `.bss` 147,352 B; code 127/128, rodata 61/64; slot 1,605,632
  B); `check-resident` ok (64 sections from 21 roots, none paged); `size
  --check` within budgets; `stack --check` margin 4,276 B; `cargo xtask
  test pagelink` passed. Budgets unchanged (a measured note under
  `stack_static_margin_bytes`). **Not run**: `brand`, `site --check`,
  `xtask test all`'s fuzz runs.
* **For the lead**: (0) Books is hidden on the owner's Home (stored
  Home-apps mask, above); (1) the open-book underruns and the intermittent
  "could not be opened" under music, and the cut-off message, before
  Books is announced; (2) the idle-Home rate; (3) check 3 with a fresh
  EPUB; (4) the abandoned-upload hang. Both windows are nearly full
  again (1 code page, 3 rodata pages free).

### 2026-11-27 — PG-2 / PG-2b on the player; ls fixes (lead)

* **PG-2 (`bdebff77`) on the player:** SRAM3 executes cold code (U-MEM-2);
  start-up log `fw: stack guard: DWT watchpoint and canary` (U-DWT-2).
  Suite 24/26: `settings_brightness` was the journey's fault (`ui top` now
  ends in ` speech=0`; `bl=N\\b`, `febacb30`), `midi` underran once. The
  `screen_reader` journey ran for real on the player (`said=Home. Music,
  button, 1 of 12`, `duck=16462` with music). Found: updates from a PG-2
  image too slow and reset by the watchdog, Etude underruns, `clk=288/288`
  at idle, idle paging up (fixed in PG-2b below).
* **PG-2b (`ba0abfda`, merged) on the player**, installed with `dev stage`
  + `dev reset` (owner's OK 2026-11-27; a PG-2 image could not install it):
  `dev update` of the same build **38.6 s** ("running … from slot B"; was
  9 min and a watchdog reset); MIDI folder with the test look **0/0/0
  underruns** (Etude, Hill, Sound tour; before 2/1/0, baseline ≈ 1),
  **1,273 line loads/s** (2,297 on PG-2, 1,570 before PG-2), `cpu=48` once
  the folder ended; idle Home with the test look **138 loads/s** (≈ 200 on
  PG-2, baseline ≈ 120); `dev suite` **25 pass, books skipped, 0 failed**.
* **Soak findings, fixed (AU-5):** a reboot reopens the last track paused,
  and the card task read test commands only between tracks, so `ls` after
  a reboot waited until a stop: the soak's state check after its reboot
  saw no files (FAIL queue). Now `ls` while paused goes to `PAUSED_CMDS`
  and the paused loops answer it (`452d0a9d`,
  `regress_ls_waited_while_a_track_was_paused`; checked on the player with
  a reopened paused track). And `ls` of a big folder lost its `(N
  entries)` footer (names filled the whole 1 KiB reply), so the soak's
  discovery reported "no answer after 18 s" for every music folder; ls
  now keeps the footer's room (`f7b1f4c8`,
  `regress_ls_of_a_big_folder_lost_its_footer`; `ls /Knower` → `(59
  entries)` on the player).
* **Soaks** (`dev soak music --minutes 15`): on `ba0abfda` FAIL queue only
  (above); on `452d0a9d` pass: stack free min 18,568 B, line loads/s mean
  1,728 / max 4,103, evictions/s max 4,124, underruns 0, card errors 0,
  uptime vs host −21 ppm, CPU idle 63 %. DAC headroom low-water −4,491
  frames fell inside seek/playdir windows (not a finding). On `f7b1f4c8`
  **PASS, 18.2 min**: discovery listed the owner's folders (16 folders,
  none "no answer"); stack free min 18,568 B, loads/s mean 1,742 / max
  4,196, evictions/s max 4,216, underruns 0, DAC headroom min 770 frames,
  card errors 0, CPU idle 62 %. Owner's settings unchanged (`settings
  changed`: `701=1 d50=10470 e80=20992`, as before the session's tests).
  The player runs `f7b1f4c8` from slot B.
* Books stays off: with it the static stack margin is 3,460 B (budget
  4,096 B); needs ≈ 0.6 KB cut or the owner's OK.

### 2026-11-27 — PG-2b: PG-2's update, MIDI, clock and idle regressions (worker, player)

Brief: the lead's four findings on PG-2 (`bdebff77`): updates broken, MIDI
underruns in "Etude", the clock "stuck" at 288 MHz after MIDI, then idle
paging, the suite and a soak. The player ran `bdebff77` from slot A
throughout; `dev backup-verify` ok; **no slot was made valid, nothing
committed** (one aborted 32 KiB partial write into slot B, one card
install that reset before it wrote anything).

* **1. Updates — cause (measured).** `cargo xtask dev prof --pc --during
  "update …edd929c4.ofw --stop-after 32768"` (new: `--stop-after` writes
  that much and sends ABORT; `update` allowed in `prof --during`): 32 KiB
  in 13.1 s = **2.5 KB/s**, **88.9 % of the samples in
  `sha2::sha256::compress256`** (cold, rodata window), pager 5,300 line
  loads/s, **129 rodata faults per 64-byte block**. In `bdebff77`
  `compress256` sits at `0x3abad0–0x3abc9a`, physical lines 14–15; its
  round loop crosses into line 15, where the round constants `K`
  (`0x35bd08`) live: every round loads K's line over the loop's code and
  the next fetch loads it back. The write took 8–9 min; the USB read-back
  reads 16 KiB per supervisor poll in the USB task (≈ 6 s at that rate), so
  thread mode stops, the watchdog task cannot feed, and the watchdog
  (5.6 s) resets — the lead's 02:00:28 reset (no fault record: a watchdog
  reset writes none). **The card path is no way round**: Settings › System
  › Update firmware with the image in `/.orpheus/update/` (put there with
  `dev put`, 59 s) reset the player ≈ 5 s after Install: the card task
  hashes 4 KiB per step (≈ 1.6 s) on the RT executor with 1 ms breaks, the
  UI loop (≈ 27 ms of work) cannot finish, `UI_BEAT` stops, no feed. **A
  PG-2 image cannot install another image**; `dev stage` (ADFU) is the only
  way, and it needs `dev reset` after it (owner's OK, the lead's rule), so
  the fixed image is not on the player.
* **1. Fixes.** (a) `resident.toml [bin.firmware.cold] exclude`: the
  installer's write/take/same/program/erase/verify_step/guard,
  `Target::allows`, the `Nor` adapter, `usb_out`/`with_usb`/`poll`,
  `install_file`, `Sha256::update` and `sha2::sha256::*` stay in the code
  window (SHA-1/512 and the rest of the update screens stay cold).
  (b) `pagelink` cold placement: a section goes at a group's end if that
  is clear of the constants it reads, else at the first clear line start
  after padding, else into a new group while the window has room; sections
  still sharing a line come back into the code window before warmer cold
  code; `layout.json` reason `cold-shared:`. Default image: **0 conflicts**
  (was 8), cold code 8 rodata pages (was 6), rodata 46/64. (c) The USB
  read-back step is bounded to 50 ms as well as 16 KiB.
* **2. MIDI — cause (measured, `bdebff77`).** `playdir
  /.orpheus/test/suite/midi` (owner's look, cable): "Etude" `underruns=2`
  (`ub: dec=2`, shed 46, `pg=` 4–10 %, `clk=288/288`), "Hill Song" 1,
  "Sound tour" 0 — fewer than the lead's 3/10 but more than the ≈ 1 event
  of the whole run before PG-2. `dev prof --faults 25` during "Etude":
  **2,297 line loads/s** (before PG-2: 1,570/s, STATUS `e1a0dc7c`), 35 % of
  the faults rodata (1,170/s), from `plan_piece`, `Engine::estimate`,
  `after_piece`, `Governor::over_budget/lower` and the outer loop's poll
  reading 12–24-byte anonymous constants (clock steps `CLOCKS_MHZ`, the
  `syn:` field tables, `match` lookup tables) that PG-2's new rodata order
  put on physical lines 5, 10 and 11 — inside the 14 lines (0–13) the
  wavetable `WAVES` takes: each piece reloads them, then the render
  reloads the wavetable. The other suspects (the canary check, the speech
  task, the code window at 127/128) were not isolated: an A/B against the
  pre-PG-2 image needs a working update first.
* **2. Fix.** `hot.toml` `resident_constants = N` (new cluster field;
  `pagelink::cluster_constants`): plain constants of ≤ N bytes (no
  pointers, not merged) that a cluster's code reads directly are resident.
  `synth-inner` and `synth-block` take 256: **28 constants, 505 B**
  resident (resident image 59,824 → 60,336 B, budget 61,440). What is left
  paged of the per-piece code's rodata is `WAVES` and panic locations
  (16 B, read only on a panic). **Not measured on the player** (the fixed
  image is not installed); no A/B against the pre-PG-2 image either (same
  reason).
* **3. Clock — cause (measured).** The clock *is* released: `dev stats`
  `clock level Base 48` 3, 10, 20 and 45 s after a mid-song `stop` and 5,
  45 and 90 s after the folder's natural end (switches stop). The
  `status` line's `clk=288/288` is the synth governor's last step (`syn:`
  field, kept after the file), not the clock; `clk=144/144` after a boot
  is the same field's initial step. And `dev stats` printed `clock level
  ?` at 288 MHz (its name table stopped at Turbo). **Fixes**: `syn: clk=`
  is the governor's step only while something plays (0 idle,
  `orpheus_synth::load::status_clock_mhz`), new `cpu=` = the clock the
  core runs at; `dev stats` names every `Level` (`Max 288`).
* **4. Idle Home (measured, `bdebff77`)**: screen on, test look, nothing
  loaded: **199–219 line loads/s** (`dev stats` 5–10 s apart; ≈ 0.9 UI
  loops/s); screen off and locked 177/s; with "Etude" loaded paused after a
  reboot 270/s. `dev prof --faults 10`: top `Fw::setting` (≈ 40/s) and
  `SettingsCache::get`: `setting` on physical code lines 1–2, `get` and
  `index` on line 2, so every one of the loop's ≈ 40 setting reads (the
  screen timers, the key timing, the screen reader's configuration —
  new in the image with PG-2) reloaded both — the 2026-11-25 idle-thrash
  bug again, by layout. **Fix**: cluster `settings-get` (`Fw::setting`,
  `SettingsCache::get`, `index`, `def`, `reader_cfg`, `screen_timeouts`:
  924 B, one line). Not measured on the player.
* **Not run** (the fixed image is not on the player): the update timing
  from the fixed image, the MIDI A/B, the idle rate after the fixes, `dev
  suite`, `dev soak music`.
* **Tests** (each fails without its fix): `xtask`
  `rules::tests::regress_update_data_path_ran_from_the_rodata_window`
  (the real `resident.toml`), `pagelink::tests::cold_code_that_conflicts_with_its_constants_comes_back_first`,
  `cold_code_goes_to_its_own_groups_clear_of_its_constants` (now: padded
  to line 9, no conflict), `pagelink::tests::regress_synth_piece_constants_on_the_wavetable_lines`,
  `rules::tests::regress_setting_reads_evicted_each_other`,
  `dev::stats_tests::regress_stats_clock_level_max_unnamed`;
  `orpheus_synth::load::tests::regress_idle_status_claimed_the_synth_clock`.
  `docs/regressions.md` rows.
* **Hardware facts** (01 §9): **U-MEM-2 answered** (SRAM3 executes: the
  lead's PG-2 run, Authenticator from the rodata window) and **U-DWT-2
  answered** (`fw: stack guard: DWT watchpoint and canary`: ≥ 1
  comparator). 02 §3.6: what must never be cold, the placement.
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` **2,149 passed,
  0 failed, 13 ignored** (203 binaries), `cargo xtask settings-matrix
  --check` (184 settings, 0 errors); fw fmt, `cargo clippy --workspace
  --release -- -D warnings` (also `-p firmware --features books` and
  `--no-default-features`), `cargo xtask build` (127 code + 46 rodata
  pages, resident 60,336 B, slot 1,482,752 B), `check-resident` ok (64
  sections, 21 roots, none paged), `size --check` within budgets,
  `stack --check` margin 16,852 B, `cargo xtask test pagelink` passed.
  Budgets unchanged.
* **For the lead — installing the fix** (needs the owner's OK for `dev
  reset`; one process at a time): `cargo xtask build` on this branch;
  `cargo xtask dev backup-verify`; `cargo xtask dev stage
  target/orpheus/firmware.yfw` (slot A, ADFU); `cargo xtask dev reset`;
  `cargo xtask dev info` (the new build id), `dev cmd 'boot status'` →
  slot A. Then, with the fixed image running:
  1. `cargo xtask dev update target/orpheus/orpheus-*.ofw` (same build into
     B): expect ≈ 40 s and "running … from slot B" (both slots valid
     again). `dev prof --pc --during "update FILE --no-reboot"` shows
     where the time goes if not.
  2. `ui test-look on`; `dev cmd 'playdir /.orpheus/test/suite/midi'`,
     `status` every ~9 s: Etude/Hill Song/Sound tour underruns against
     today's 2/1/0 and the pre-PG-2 ≈ 1 event; `dev stats` twice during
     Etude: loads/s against 2,297 (now) and 1,570 (pre-PG-2); `status`
     `cpu=` 288 while it plays, `clk=0/288 cpu=48` once stopped.
  3. Idle Home with the test look: `dev stats` twice 10 s apart against
     ≈ 200/s now and ≈ 120/s (PG-1).
  4. `cargo xtask dev suite`; `cargo xtask dev soak music --minutes 15`.

### 2026-11-27 — PG-2: cold code in the rodata window; speech in the image, Books fits (worker, no hardware)

Brief: grow the room for paged code so `speech` and `books` both fit,
hot paths intact; turn them on if every gate passes. Nothing run on the
player (the brief). Design and as-built: 02 §3.6, §3.1, §1.2.

* **Why not a bigger code window**: SRAM2/3 alias only inside their 1 MiB
  (FINDINGS §7: `0x400000`, `0x800000`, `0x1000000` unmapped; 01 §9
  U-MEM-4); the window registers' effect is unknown (`memwin.c` results
  unrecorded). The VA budget was the limit: 128 code + 48 rodata pages =
  1.41 MB against ≈ 1.53 MB of paged code and constants with both
  features. Size cuts alone could not close 158 KB (opt-level `"z"` had
  given 14.6 KB for 14 crates, PG-1).
* **The rodata window grows to 64 pages.** MPU region 0 now denies all of
  `0x200000–0x3FFFFF`; region 7 grants SRAM3's RAM half at `0x304000`
  (16 KiB, read/write, execute-never) instead of the 256 KiB hole that cost
  16 pages. `RO_WIN` = `0x300000`. Region 7 was the stack guard: a paged
  image keeps a **canary** (the guard's 256 B of paint, checked by the
  watchdog task before each feed; broken → a stack-guard fault record)
  and a **DWT write watchpoint** on it when the core has a comparator
  (U-DWT-2; DebugMonitor is recorded as a stack-guard fault); images
  without the pager keep the MPU guard. The start-up log says which.
* **Cold code** (`resident.toml` `[bin.firmware.cold]`, coldest first):
  Books' conversion, updates, the authenticator and its hashes, the
  library build, rare settings screens, the rest of Books, the developer
  channel's commands and replies. Never a cluster's code; `xtask stack
  --check` fails if a cluster's path reaches cold code
  (`a_path_into_cold_code_fails_the_check`). **Only as much leaves the
  code window as it needs**: pagelink places all of it, then brings the
  warmest back while one code page stays free (`place_cold`); in the
  rodata window it packs `.px_gN` groups after the constants, first fit,
  avoiding the physical lines of constants a section refers to; header
  `+0x50` `rodata_code_pages`; the pager maps those pages executable.
* **Line conflicts**: a cold instruction whose constant maps onto its own
  physical line gets `Action::Load { bounce: true }`: the handler reads
  the instruction (and the rest of an IT block) from the page store,
  builds a trampoline in a per-depth slot (5 × 32 B, `.uninit`), loads
  the line and returns into the trampoline, which jumps back after it
  (`orpheus_pager::bounce`, host-tested against `llvm-mc` encodings;
  `xtask pagelink` audits every load of cold code with the same decoder:
  **0 of 1,290** (default) / **0 of 5,313** (with Books) cannot be
  bounced; the audit found `popcc {r7, pc}` ending an IT block in
  `orpheus_library::format::rd16`, now allowed). `status` gains
  `bounces=`. HAL feature `cold-code` (firmware only): the norprog payload
  keeps the old fatal path (it had 12 B left; its `.bss` also moved to its
  free scratch area).
* **Speech on by default** (`fw/firmware` `default = ["speech"]`): every
  gate passes. Books stays a feature: it fits and passes alone, but with
  speech the static stack margin is **3,460 B < 4,096 B** (`.bss` +23 KB
  moves the stack; the nested worst case is the UI's OTP path 5,280 B +
  the card task's Books save 8,264 B); and the suite's Home journeys need
  the Books update the BOOK-1/2 entry lists. Decision for the lead/owner:
  a 0.6 KB stack cut (three 256 B steps of `.bss`+resident, or the Books
  save/step futures, or the authenticator's HMAC frame) or a 3 KiB
  ratchet. And the room is thin with both: 1 code page + 2 rodata pages
  free (≈ 24 KB of window), against 58 KB + 152 KB with speech alone.
* **Stack cuts found on the way** (measured with the features on, first
  time a paged speech image existed): the speech speaker was built in its
  task (`new_speaker` frame 7,640 B, thread context) — now in `main`
  before the executors (`speech::init`, start-up context 12,368 B covers
  it); Books' `epub_head` built the EPUB job twice (struct update, 5,784
  B) and `begin_scan` through a temporary (4,544 B) — now in place (0 and
  448 B). The pager context 236 → 248 B (the trampoline built straight
  into its slot).
* **Numbers** (`xtask build`/`size`/`stack`): default (speech): resident
  59,824 B (text 30,656, rodata 12,480, data 16,688), `.bss` 134,216 B,
  code **127/128** pages (8,202 B free: the reserve), rodata **45/64** (7
  cold pages, 49,178 B, 11 sections share a line with a constant they
  use), slot 1,474,560 B, stack 14,636 B of 32,000 usable (margin 17,364
  B), check-resident ok (64 sections from 21 roots, none paged). With all
  of `[cold]` out (before the adaptive placement) the same image took
  121/128 + 51/64. Speech + Books: resident 60,172 B, `.bss` 147,456 B,
  code **127/128**, rodata **62/64** (20 cold pages, 163,130 B), slot
  1,613,824 B, stack 14,716 of 18,176 B (margin 3,460 B), check-resident
  ok, size ok. Books alone (all cold out): code 116/128, rodata 57/64,
  `.bss` 137,720 B, margin 14,816 B. Clusters
  unchanged in the code window; `games-io` regenerated for Books
  (`flush_step`, `rewrite`: 14 of 14 lines); `synth-render` 18 of 18,
  32 of 32 with its `fits_with` (speech's mixer and `PcmOut::start`
  stopped: they run only while speaking / once per DAC start). Budgets:
  `resident_bytes` 72 → 60 KiB, `bss_bytes` 132 → 144 KiB (sum
  unchanged), `rodata_pages` 48 → 64.
* **Tooling**: `?pattern` in `hot.toml` `stop` and `stack.toml` targets
  may match nothing (a feature's code); stack.toml gains the speech task
  and the reader's `dyn ListSource` calls. **Cargo keeps the other link
  when the feature set changes back** (it reported the speech+books build
  fresh and pagelink re-linked the Books-only objects): `touch
  fw/firmware/src/main.rs` before `xtask build --features …` after a
  build with other features.
* **Tests**: `orpheus-pager` `bounce` (7: encodings checked with
  `llvm-mc`, IT blocks, returns, refusals), traces
  `cold_pages_are_executable_and_constant_pages_are_not`,
  `cold_code_reading_its_own_physical_line_bounces`,
  `cold_program_traces_keep_the_invariants`; `xtask`
  `pagelink::tests::cold_code_goes_to_its_own_groups_clear_of_its_constants`,
  **`regress_speech_and_books_did_not_fit_the_code_window`** (the build's
  paged sections in `tests/regress/pagelink/`), `hot::tests::a_path_into_cold_code_fails_the_check`,
  `rules::tests::optional_patterns`; slot header round trip with the new
  field.
* **Gate**: host `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -- -D warnings`; `cargo test --workspace` **2,143 passed,
  0 failed, 13 ignored** (203 binaries); `cargo xtask settings-matrix
  --check` (184 settings, 13,248/13,295 cells, 0 errors). fw `cargo fmt
  --all --check`; `cargo clippy --workspace --release -- -D warnings`,
  also `-p firmware --features books` and `--no-default-features`;
  `cargo xtask build` (default: 127 code + 45 rodata pages, resident
  59,824 B, payloads ok), `check-resident` ok, `size --check` within
  budgets, `stack --check` margin 17,364 B; `--features books`: build,
  check-resident and size ok, **`stack --check` fails on the margin
  (3,460 B)** — the reason Books stays off; `cargo xtask test pagelink`
  (the paged toy end to end) passed. **Not run**: every `xtask dev` step
  (the brief), `brand`/`site --check`.
* **Lead's checks on the player** (`cargo xtask dev backup-verify`, then
  `cargo xtask dev update` with the default build):
  1. **U-MEM-2 first**: right after the update, open the Authenticator
     from Home (its screen, `AuthScreen::reload`, and the OTP code are
     cold in the default image) — that is cold code running from SRAM3
     (the default image's cold set: the authenticator, updates, the
     library build, the HID keyboard, their hashes; `layout.json`
     `reason: cold:…` lists them). If the
     player instead reboots with a fault record whose PC is in
     `0x30xxxx–0x3Fxxxx`, SRAM3 cannot execute: stage 2 falls back to the
     old slot after three starts; report the record. `cargo xtask dev cmd
     status` shows `bounces=N` (before `pos=`).
  2. `cargo xtask dev results` (the log ring): `fw: stack guard: DWT watchpoint and canary`
     or `… canary` (U-DWT-2).
  3. `cargo xtask dev suite` — all journeys; **`screen_reader` now runs**
     (reader on through the session, `said=` after each move, `vol=2`,
     `duck=16462`, "Screen reader off"); `books` is skipped (not in this
     image). Then TTS-3's listening and speech checks 2–4 (its entry).
  4. MIDI: `cargo xtask dev cmd 'playdir /.orpheus/test/suite/midi'`,
     `cargo xtask dev cmd status` every ~9 s: compare with ≈ 1 underrun
     event over the three songs; `bounces=` stays where it was (no cold
     code runs during MIDI unless a library scan does).
  5. MP3 soak (`cargo xtask dev soak mixed` or the usual): loads/s within
     `[soak]`, 0 underruns outside the windows, `pager_load_errors` 0.
  6. `cargo xtask dev stats` at idle Home: pager loads/s against ≈ 120/s
     (PG-1); then open Settings › Accessibility and the authenticator
     (cold code: a few loads, `bounces` may rise by a few).
  7. Books (only after the stack decision): `touch fw/firmware/src/main.rs;
     cargo xtask build --features books`, then BOOK-1/2's checks.

### 2026-11-27 — owner: chords approved, WAVs sound great (lead)

* Owner 2026-11-27: "Those key combos work for me. Example wavs sounded
  great." The screen reader's toggle (Back + Menu held 2 s) and
  where-am-I (Back + OK) replace the spec's Vol+/Vol− chord (05 §7.14);
  the listening checks for TTS-2, SYN-2, MIDI-1, SEQ-1/2 and A11Y-1 pass.
### 2026-11-27 — TTS-3 / A11Y-1: speech and the screen reader on the player, behind a feature that does not fit yet (worker, no hardware)

Brief: a speech voice in the firmware (16 kHz alone, mixed over music,
speech volume, ducking or pausing, D-4 clock rules, the synth's cache set
intact), the `SpeechService`, the reader and chords in the UI task, the
`screen-reader` feature on if the budgets allow, observables and a journey,
host measurements. Nothing run on the player (the brief). Full "as built"
in 04 §8 and 05 §7.14 ("As built (TTS-3)").

* **The budgets do not allow it: the feature is built but off by
  default.** Firmware feature `speech` (screen reader + voice; `cargo xtask
  build --features speech`) needs **133 of the code window's 128 pages**:
  paged code 1,013,760 → 1,089,102 B (+75.3 KB; the speech engine ≈ 45 KB,
  the reader's describe/observe ≈ 13 KB, firmware glue ≈ 14 KB), 40.5 KB
  over. opt-level "z" does not close it (orpheus-tts + orpheus-apps: 28.6
  KB over; + PG-1's 13 cold crates 19.1; + 6 more 13.8; + orpheus-ui 8.6 KB
  over; none kept: the UI's and the MIDI path's speed). **Owner decision
  wanted** (user check 36): what makes ≈ 41 KB of paged code room — e.g. a
  format or app compiled out (Opus ≈ 61 KB, the games ≈ 55 KB with their
  screens and card glue, the MIDI player and synth ≈ 65 KB, pictures
  ≈ 45 KB), or a pager change so cold code may run from the rodata window
  (18 of its 48 pages free, ≈ 147 KB; 02 §3). The default build is as
  before (124 pages, resident 56,044 B).
* **Speech voice** (`fw/firmware/src/speech.rs`, `audio/speech.rs`): the
  speech task (thread mode) renders `Speaker` at 16 kHz into a 128 ms ring
  in 256-sample chunks once half of it is free; the card task mixes it
  after the DSP chain at the DAC's rate (`orpheus_tts::mix::Mixer`:
  resampler, speech volume, the music lowered) or, with nothing playing
  (idle, paused, the card with USB), runs the DAC at 16 kHz itself
  (started after 32 ms, the ring cleared ahead of the writer, a stop taken
  back and faded with `Book::rewind`, stopped when played out). Clock floor
  Mid while speaking (`governor::with_speech`; Automatic only).
* **Screen reader in the UI task** (module `ui::reader`): chords on the raw
  edges, `KeyGate::reader_keys`, `Config::timeouts`, `Shell::describe` +
  `Reader::observe` after each key phase and each loop, toggle Back + Menu
  2 s (`a11y::TOGGLE_KEYS`, one constant; owner decision pending, user
  check 34), where am I Back + OK, Music while speaking › Paused
  (`services::SpeechPause`).
* **New settings**: Speech volume `0x100C` (Normal, −6, −12, −18 dB) and
  Music while speaking `0x100D` (Lowered, Paused, Unchanged) in the word
  `0x0D61` (default unchanged); effects entries, matrix rows `s100c`,
  `s100d`, a new service context `speech-music` (Rotate, Home screen,
  Language and the reader's settings show there; declared).
* **Lexicon paged**: one 32,766 B static was larger than a rodata group
  (pagelink refuses it): three pieces cut at block starts
  (`regress_lexicon_larger_than_a_rodata_group`).
* **Observables**: `ui speech` (sounding, alone, queued, ring, vol, music,
  duck, counters, `said=` last text cut at 64 B); `ui top` ends with
  `speech=0|1`; the simulator player answers both. Journey `screen_reader`
  with a new journey field `needs` (`ui speech ~ ^speech sounding=`):
  skipped, not failed, on a build without the feature.
* **Numbers** (`budgets.toml [tts]`): RAM .bss +9,736 B (≈ 134,030 of
  135,168 B), .data +276 B; host bench synthesiser 31.3 ns per 16 kHz
  sample (TTS-2: 31), mixer 13.2 ns per 48 kHz frame, 15.9 at 44.1 kHz,
  2.1 alone; player estimate per second of speech: alone ≈ 6.0 M cycles
  (6 % of Mid), over 48 kHz ≈ 11.9 M (12 % of Mid; 6.2 M of it in the card
  task), over 44.1 kHz ≈ 14.3 M (TTS-2: 6 / 10.8 / 13 M). Stack and
  check-resident of the speech build: not measurable (no paged image).
* **Tests**: `orpheus-tts` `mix` (9), `orpheus-audio` `rewind_takes_back…`,
  `orpheus-power` `speech_floor_is_mid_and_only_in_automatic`,
  `orpheus-apps` `speech_pause` (3), the lexicon regression; sim
  `speech_volume_scales_the_speech`, `music_while_speaking_lowers_or_pauses`;
  goldens `app_group12_dark_latin`, `app_group12_light_cjk` (13 rows: the
  scroll thumb; looked at), spoken golden `speech_rate` ("of 13");
  `tests/suite/expected/screen_reader/home.png`.
* **Gate**: host `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -D warnings`, and `-p orpheus-ui` / `-p orpheus-apps`
  without features; `cargo test --workspace` 2,121 passed, 2 failed, 13
  ignored — both failures mine and fixed (a test count my edit changed in
  `bricks_packed_settings`; the soak test's copy of the session
  whitelist), each re-run and passing: 2,123 / 0 / 13; `cargo xtask
  settings-matrix --check` (184 settings, 13,248/13,295 cells, 0 errors);
  fw fmt, `cargo clippy --workspace -D warnings` with and without
  `--features firmware/speech`; `cargo xtask build` (default: 124 code +
  30 rodata pages, resident 56,044 B), `check-resident` (ok, 59 sections
  from 21 roots, none paged), `size --check` (resident 56,041, bss 124,296
  B), `stack --check` (margin 31,044 B). With `--features speech`: builds
  and links pass 1; pagelink refuses it (133 pages), so check-resident,
  size and stack of that build were **not run**. **Not run**: every `xtask
  dev` step (the brief), `brand`/`site --check`.
* **Open**: (1) the room decision above; (2) no `hot.toml` cluster for the
  speech path (no layout to measure: ≈ 12 per-sample lines evict as many
  music lines per chunk, ≈ 8 chunks a second); (3) speech waits while
  recording (the card task is in the recorder; the floor stays Mid) and
  does not sound over the radio; (4) over music up to 64 ms of speech
  already mixed plays after a stop; (5) first-sound latency, cycles and the
  golden hashes on the player.
* **Lead's checks on the player** — only with a build that fits (after the
  owner's decision; then `cargo xtask dev backup-verify`, `cargo xtask dev
  update` with `cargo xtask build --features speech`):
  1. `cargo xtask dev suite` (all journeys; `screen_reader` runs instead of
     being skipped: the reader on through the session, `said=` after each
     move, `vol=2`, `duck=16462` over the fixtures' music, "Screen reader
     off").
  2. Listen with the owner: hold Back + Menu 2 s on Home (reader on: the
     rising earcon, "Screen reader on", "Home. Music, button, 1 of 12");
     move through Home, Settings › Accessibility, a list in Music; Back +
     OK held (where am I), held 2 s (time and battery); a key during speech
     cuts it within ≈ 10 ms alone; Speech rate 2× and 0.5×; Voice; Speech
     volume −12 dB; the toggle again (falling earcon, "Screen reader off").
  3. Underruns while speaking over an MP3: Automatic, the cable in, play
     the suite's music folder from Now Playing, move through a list for a
     minute with the reader on; `cargo xtask dev cmd status` every ~9 s:
     underruns 0, `pg=` and the `pcm: head` lines in the log; then the same
     with Music while speaking Paused (the music pauses for each utterance
     and resumes ≈ 0.4 s after) and over a MIDI file (Etude) — any
     underrun: its `ub=` label and `ui speech` `late=`.
  4. `cargo xtask dev stats` while speaking alone and over the MP3: the
     clock at Mid or above in Automatic (192 MHz during MIDI as before),
     back to the idle level within ≈ 3 s after the speech; in Battery saver
     the level stays pinned and speech may gap (`late=` grows), nothing
     hangs.
### 2026-11-26 — BOOK-1/2 wiring: Books in the firmware, behind a feature (worker, no hardware)

Brief: Books on the player (the `Shelf` split, card work as paced chores,
the EPUB's inflate window from the decoder arena, the fonts, Books on,
observables and a suite journey). Nothing run on the player (the brief).
Design and as-built: 05 §7.15 "As built (BOOK-1/2 wiring)".

* **Books does not fit the code window: it is built behind the firmware
  feature `books`, off.** With it the paged code needs **139 of the 128
  pages** (1,131,550 B of 1,048,576 B: 82,974 B over; the window had
  33 KB free). Books is ≈ 112 KB of code: `orpheus-books` 92 KB (the
  shelf's step state machines 25 KB, the converter 21 KB, EPUB 13 KB, OPF
  6.7 KB, layout 4.7 KB, FB2 4.5 KB, inflate 3.2 KB …), its screens 12 KB,
  the firmware module 8 KB. Measured, not kept: `opt-level = "z"` for
  `orpheus-books` saves 10 KB (138 pages). The shipping image is unchanged
  apart from no-op hooks (below); `cargo xtask build --features books`
  builds the Books image (it stops at pagelink today);
  `App::Books.on_player()` stays false; the suite's `books` journey is
  skipped on an image without it.
* **Measured with Books in a linked image** (Opus, Speex and Vorbis left
  out for the measurement only, `orpheus-codecs` features `wav,mp3,flac`;
  not committed): 124 code + 28 rodata pages, `check-resident` ok,
  **stack**: card task (RT) 8,144 → **11,048 B** (+2,904: a Books step under
  the idle loop's request serving), UI task unchanged (5,188 B), nested
  worst case 17,404 B of 32,768 B usable (margin 15,364 B; usable falls
  with `.bss`); `stack --check` with Books has one problem of its own: the
  `games-io` cluster's `Volume::flush_step` and `rewrite` are now shared
  with Books' code and placed outside it (15 of 14 lines: `cargo xtask hot
  --name games-io --write` when Books goes in). **`.bss` +13,240 B** (all
  codecs: 137,544 B against the 135,168 B budget: 2,376 B over; with the
  speech engine's 8 KiB more). The 20 px face (`book-fonts`, 20 KB, paged
  since PG-1) comes with the feature; rodata pages are not the limit.
* **Options for the lead / owner** (not decided here): (1) a pager change —
  cold code in the rodata alias window's free pages (18 of 48, 144 KiB:
  Books' code would fit; execute from the two SRAM3 slots the fonts use,
  so the line thrash needs measuring) — a package of its own; (2) a code
  size package across the firmware (the `firmware` crate is 295 KB of code,
  `orpheus-apps` 150 KB: async state machines); (3) a codec out of the
  image (Opus + Speex + Vorbis ≈ 96 KB) — the owner's decision. RAM then:
  `resident_bytes` has 17 KB room, so moving 8 KiB of it to `bss_bytes`
  (their sum unchanged) covers Books; with the speech engine too it needs
  ≈ 10 KB more.
* **`orpheus-books`, the split**: `Shelf` (the screens' model, **5,476 B**)
  changes only on requests; what a request asks of the card waits in it
  (open, save, close, drop the open book's files or its layout). `Desk`
  (the card side, **7,712 B**) holds the job, the files, a 1 KiB I/O
  buffer and a directory entry; `Desk::step_with(&mut Shelf, …)` is one
  step; `Books` keeps both (simulator, tests: `Deref` to the model). In all
  13,192 B (was 12,160 B in one `Shelf`; the search's directory entry is
  the desk's). Step future **5,464 → 2,600 B** (host; the steps' buffers in
  the desk: index chunks, the layout's last write, the library header,
  records and the reading state; the rest is mostly the file system's own
  futures: a path walk's open is 1,608 B). On the player: `step_with`'s
  poll frame 10,040 → 2,568 B (each job, the EPUB job and the readers built
  in a function of its own), `save_state` 3,072 B → small, `Shelf::request`
  3,992 + 1,360 B (a `Book` built and moved for Open) → in place
  (`Book::reset`, `BookState::clear`, `LibRecord::clear`), `Desk::reset` in
  place (7.7 KB).
* **Card work rules** (`orpheus-books`): `Desk::mkdir` — off while music
  plays: a folder that exists is looked up (`open_dir`), a missing one
  waits (`Desk::wants_mkdir`, the step reports nothing done, nothing
  written); `Desk::next_kind` (`Kind`: write, search, read, conversion,
  layout) for the chore costs; `Desk::suspend` / `Shelf::card_back` for the
  USB hand-over; `Scope::Test` (`/.orpheus/test/suite/books`).
* **Firmware** (`fw/firmware/src/books.rs`, `books/on.rs`; feature `books`;
  without it the functions are no-ops and the card task and UI call them
  unconditionally): the model and the desk in two statics; the UI takes
  the model at its first use in a UI loop and gives it back at the loop's
  end (`ui_release` in `ui.rs`); the card task takes it per step (chores
  only try; elsewhere it waits for the UI). While a track plays: chore
  kinds `job:bwrite`, `bscan`, `bread`, `bconv`, `blay` (`media::JOB_KINDS`
  7 → 12; seeds 20, 20, 15, 15, 15 ms) after the listing pieces,
  `audio::piece_left` before each step, no folder made, no scratch.
  Paused: steps between the paused loop's requests (≤ 150 ms a pass, a
  rest as long as each step), folders made, no scratch. Idle:
  `books::run_idle` before anything else in the pass (and on `Req::Books`,
  `REQ_KINDS` 16 → 17, which wakes the idle loop), the arena's first
  35,072 B lent while `Desk::needs_scratch`; every idle pass counts as the
  arena given away (`books::arena_taken`), and a lent step after one is
  first a step without it (an entry half decoded starts again rather than
  inflating from a window something else overwrote). Data tag 0x102 →
  `other::BOOKS`. With the feature, Home shows Books (`app_available`;
  `App::on_player`, which the host tools use for the shipping image,
  does not). USB hand-over: `Server::card_leaving` writes the reading
  state and the jobs' files first; power-off and restart close the reader
  (position written, ≤ 1 s). The test look reads Books' settings
  0x0F01–0x0F07 as their defaults.
* **The EPUB hint** is **"Stop the music to prepare this book."**, not
  "Pause …": a paused track keeps its decoder in the arena (the play loop
  stays in `pause`), so only Stop frees the window (new string
  `BooksStopMusic`, above the `seq` block: numbers unchanged). The reader's
  waiting state shows it and the screen reader says it; golden
  `books_epub_waiting` (dark, light; looked at).
* **Observables** (developer channel, read-only for the card): `books
  status` → `books: lib=ready n=2 found=2 book=ready conv=1000 wait=0 top=…
  page=0 pages=4 laid=1 steps=… lent=… retaken=… mkdir=0 test=1` (the model
  as the card task left it; `books: not in this build` without the
  feature); `books test on|off` (the suite's folder is the library, RAM).
* **Suite**: journeys may name `requires = "CMD ~ REGEX"`: skipped (and
  passing, "skip" in the report) when the reply does not match; journey
  **`books`** (`setup`; `requires = "books status ~ ^books: lib="`): the
  library (the text focused; progress masked), the text's first page (three
  Prevs from wherever a run left it), a page turned (`books status`
  page=1), back, library, Home. `--setup` uploads
  `/.orpheus/test/suite/books/test-card.txt` (our own text, four pages) and
  `lighthouse.epub` (the simulator's EPUB sample); the simulator's
  `set:bookstest=on` writes the same files. Expected frames `books/library`,
  `page1`, `page2` (looked at). When Books goes into the image: the suite's
  appearance gets `set:books=on`, `home` takes `next shot:home_books`
  before Settings, `games` starts with `prev*3`, this journey's `sim:
  set:books=on/off` go, and the expected frames are blessed again.
* **`xtask stack`**: `[[indirect]]` entries may be `optional` (callers all
  absent: skipped); `stack.toml` has one for Books' `dyn Sink` and
  `dyn Metrics` calls.
* **Tests**: `orpheus-books` `tests/shelf.rs` `split_on_fat32`,
  `split_on_exfat` (the model and the desk apart: no folder made while
  "music plays", the work waits with nothing written; library, conversion,
  layout and pages with ≤ 12 sectors written in any step — measured 11 on
  FAT32 and 8 on exFAT: the search's last step and conversion writes on
  512-byte-cluster images; `next_kind` none ⇒ nothing done; `suspend`
  writes the position, leaves no dirty sector and the book reopens at the
  same top; fsck clean); the step-future ratchet (≤ 2,816 B); `ram.rs`
  ratchets (`Books` 13,248, `Shelf` 5,504, `Desk` 7,744 B);
  `shelf::tests::book_reset_is_a_fresh_book`; `xtask`
  `stack::tests::optional_indirect_entries_may_be_absent`,
  `suite::tests::a_journey_the_image_cannot_run_is_skipped`; the
  simulator's Books tests and goldens (one new pair) and the suite's
  simulator run (`suite_passes_on_the_simulator`) pass. No bug found in
  shipped code (no `regress_` test): the frames above were Books-only.
* **Default build** (Books off): resident 56,036 B (unchanged), `.bss`
  124,304 B (+8: the chore cost table's six new kinds), 124 code + 30
  rodata pages, stack margin 31,068 B (unchanged).
* **Gate**: host `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -- -D warnings` clean, and `orpheus-apps` alone with
  features none, `books`, `games,books`; `cargo test --workspace` **2,109
  passed, 0 failed, 13 ignored** (203 binaries; fuzz corpora replayed);
  `cargo xtask settings-matrix --check` (182 settings, 0 errors). fw: `cargo
  fmt --all --check`; `cargo clippy --workspace --release -- -D warnings`
  and `-p firmware --features books` clean; `cargo xtask build` ok
  (resident 56,036 B, 124 code + 30 rodata pages, slot 1,323,008 B);
  `check-resident` ok (59 sections from 21 roots, none paged); `size
  --check` within budgets (resident 56,033 B, `.bss` 124,304 B); `stack
  --check` margin 31,068 B. **Not run**: any `xtask dev` step (the brief),
  `brand`, `site --check`, `xtask test all`'s cargo-fuzz runs (nightly).
  The root disk filled during the run (other worktrees' builds): my
  incremental caches and `fw/target` were deleted to finish.
* **Lead's checks on the player.** With the shipping image nothing of
  Books runs; `cargo xtask dev suite` should pass as before with `books`
  reported as skipped (`books status` answers `books: not in this build`).
  With a Books image (once there is room; for a trial, the measurement
  build above: `orpheus-codecs` without Opus, Speex and Vorbis, `cargo
  xtask build --features books` — `stack --check` then also reports the
  missing `ogg` callers and the `games-io` cluster; the owner's decision
  whether to install it):
  1. `cargo xtask dev suite --setup books`: library, page 1, page 2 as
     expected; `books status` lib=ready n=2, page=0 then page=1.
  2. Underruns while opening and converting during MP3: play an MP3
     folder of the owner's music, `cargo xtask dev cmd 'books test on'`,
     open Books, open "The Test Card" (deletes nothing; its cache under
     `/.orpheus/cache/books/`), turn pages, then `cargo xtask dev cmd
     status` every ~5 s: `underruns=0`; `chore:` lines name `job:bread`,
     `job:bconv`, `job:blay`, `job:bwrite` with their budgets (a first
     run on a card without `/.orpheus/books` waits in "Looking for books"
     until the music stops: `books status` mkdir=1).
  3. The EPUB while music plays: open "The Lighthouse Keeper": the reader
     shows "Stop the music to prepare this book." (`books status` wait=1,
     lent= unchanged); `cargo xtask dev cmd stop`: within a few seconds
     conv rises to 1000 and the first page shows (lent= grows, retaken=
     1 at most); play again and turn pages (no conversion: cached).

### 2026-11-27 — PG-1 on the player; games checked by the owner (lead)

* PG-1 (`5cba1d97`: resident 81,876 → 56,036 B) on the player: suite all
  journeys but `midi` passed (idle Home 122 line loads/s, was ≈ 700; stack
  free 32,912 B); `midi` failed only on a stale check (`vlim=16 over=0`:
  Etude sheds at 288 by design, D-4) — the check now asks for voices and no
  underruns only, and passes on the player. MIDI run: one underrun event
  over the three songs, `pg=` 2–6 % (the paged UI font costs nothing
  visible).
* The owner changed Salvo's settings by hand (`e80=20992` stored); the
  settings baseline for test runs is now `701=1 d50=10470 e80=20992`.
* **Owner 2026-11-27: games tested by hand, "all worked well"**; **MIDI
  files played by hand, "all worked well as well"** (user checks 35 and 33).

### 2026-11-26 — PG-1: resident room (the UI font and the merged strings paged; the code window's gaps filled) (worker, no hardware)

Brief: resident 81,875 of 81,920 B (45 B left), code window 125/128
pages, and `pagelink` warned on every build that `.rodata..Lanon.<hash>.52`
(17,794 B) "is larger than a rodata group: resident". Nothing run on the
player (the brief).

* **The 17,794 B section** was the **UI font** (`orpheus_ui::font::UI`,
  `ui12.yfnt` through `include_bytes!`: its only referrer in the
  relocation graph), not the MP3 Huffman tables that `budgets.toml`'s
  2026-11-24 note named. It sat in the resident `.rodata` in SRAM0:
  17.8 KB of the resident budget. Fix: built-in fonts are linked as two
  statics of at most 16 KiB (`builtin_font!`: `split_point` cuts at the
  last glyph boundary at or below 16 KiB; header, ranges and records stay
  in the first piece), `Font::glyph` reads a bitmap from either. UI font
  16,381 + 1,413 B (the tail: 110 rare glyphs, U+04C7… and U+2248…U+25C2,
  arrows and triangles), `BOOK20` (feature `book-fonts`) 16,372 + 3,872 B
  and now pageable too; the other faces fit one piece (an empty tail).
  Rendering is unchanged (every glyph of every face compared with the
  whole file's). `pagelink` now **fails** on rodata larger than a group
  that no rule places, instead of warning.
* **The linker's merged strings** (`.rodata.str1.1`, 8,042 B,
  "linker-generated", resident): panic-location file names (rustc emits
  them NUL-terminated, so LLVM puts them in the mergeable C-string
  section) and `core`'s panic message pieces. Only paged sections refer to
  them (2,160 referrers, no resident one). `pagelink` now pages a merged
  section when no root reaches it, no rule keeps it and every referrer is
  paged; the pass-2 script places it whole like any input section. The
  constant pools (`.rodata.cst4/8/16/32`, 3,836 B) are kept resident by a
  new `resident.toml` rule: MP3 `decode`, Opus CELT and the MIDI player
  read them every frame, and `main` reads `cst32` before the pager runs.
* **Code window**: the line before each cluster's line-aligned start is
  filled with code from no cluster, largest first (7,458 B; the clusters'
  addresses and extents are unchanged: every cluster is at the same VA as
  before, `fits_with` and the line checks see the same layout). 125 → 124
  pages, 33,114 B free (was 25,674). Measured and not kept: opt-level
  `"z"` for 14 cold crates (apps, img, otp, usb, embassy-usb, library,
  meta, bt, rec, hid, update, sha1, sha2, fm) saves only
  14.6 KB of code (1 page) and adds 5.4 KB of rodata, at a speed cost
  in the UI that the MIDI path is sensitive to. The large paged
  diagnostics (developer-channel handlers, `status_reply`s,
  `test_command`s: 28.4 KB) are what the lead's tools use; not touched.
* **Audit of what is still resident** (56,036 B: text 28,304, rodata
  11,325, data 16,404): the MP3 decoder 13.9 KB (rule, per-frame path),
  compiler builtins 5.0 KB (the whole archive by rule; `u64_div_rem`
  864 B and the float conversions might be paged after a profile), the
  DSP process functions 5.9 KB, the cst pools 3.8 KB, `.text.resident.*`
  3.0 KB (pager fault path, `nor::writable`), `main` 1.2 KB, the roots'
  closure ≈ 2.5 KB. `.data` is 16.4 KB of statics with non-zero initial
  values (`audio::SERVER` 4,368, `media::BROWSE`/`services::UI_BROWSE`
  3,616 each, `media::TEXT`/`services::UI_TEXT` 1,556 each): RAM state;
  zero-initialised it would move to `.bss` and free no RAM. No `core::fmt`
  and no panic site in the resident image (`resident_panic_sites` 0).
* **Numbers** (thumbv7em, `xtask build`/`size`/`stack`): resident 81,876
  → **56,036 B** (`size` 56,033; rodata 37,167 → 11,325), `.bss` 124,296
  (unchanged), code pages 125 → **124** (33,114 B free), rodata 28 →
  30 of 48, slot 1,339,392 → 1,323,008 B, stack usable 45,568 B,
  margin 5,212 → **31,068 B**; `synth-render` 18 of 18 and **32 of 32**
  with its `fits_with` (unchanged), `games-io` 14 of 14 (31 of 32).
  Budgets: `resident_bytes` 80 → 72 KiB, `bss_bytes` 124 → 132 KiB
  (their sum unchanged: the stack's floor is what it was).
* **Rodata cache** (16 lines, direct-mapped by offset in a 16 KiB group):
  the UI font's first piece is alone in group 12; ASCII text touches its
  lines 0–1 (header, ranges, ASCII records) and 6–7 (ASCII bitmaps).
  During MIDI the wavetable (`WAVES`, group 11) holds lines 0–13, so Now
  Playing's once-a-second times (UI font) now evict ≈ 3 wavetable lines a
  second (≈ 6 line loads/s, ≈ 1 ms/s); `UI` and `UI_BOLD` share line 0
  (a screen with both faces reloads it once a redraw).
* **Tests**: `orpheus_ui::font::tests::regress_ui_font_resident_as_one_oversized_section`,
  `split_point_respects_glyphs`; `xtask` `pagelink::tests::regress_oversized_ui_font_was_resident`
  (the 17,794 B section stops the link; the 16,381 + 1,413 B pieces are
  paged), `merged_sections_paged_when_only_paged_code_uses_them`,
  `code_before_a_cluster_start_fills_the_line`;
  `starts_over_pads_to_the_named_clusters_line` updated (the padding is
  not filled). `docs/regressions.md` row.
* **Gate**: host fmt, clippy `-D warnings` (all targets), `cargo test
  --workspace` 2,069 passed, 0 failed, 13 ignored (synth goldens
  unchanged), `settings-matrix --check` (173 settings, 0 errors),
  `xtask test pagelink` (the paged toy end to end: passed); fw fmt,
  clippy, `xtask build` (124 code + 30 rodata pages, no warning),
  `check-resident` (ok, 59 sections from 21 roots, none paged),
  `size --check` (within budgets), `stack --check` (margin 31,068 B).
  **Not run**: any `xtask dev` step (the brief), `brand`/`site --check`.
* **Lead's checks on the player** (`cargo xtask dev backup-verify`, then
  `cargo xtask dev update` with this build):
  1. `cargo xtask dev suite`: every journey and shot as before (the text
     is drawn from the paged font now; the goldens must not change).
  2. MIDI, as in the last run (Automatic, cable, test look on, Now
     Playing): `cargo xtask dev cmd 'playdir /.orpheus/test/suite/midi'`,
     `cargo xtask dev cmd status` every ~9 s. Compare with today's ≈ 1
     underrun event in the whole run (Etude `ub=synth:rest:5ms` once,
     Hill Song `underruns=2`, Sound tour 0), `pg=` 0–6 %, pager loads
     during Etude ≈ 1,570/s. Expected: the same within noise (≈ +6 line
     loads/s). More than 2 events in a song: their `pcm: head` labels and
     the `syn:` line.
  3. MP3 soak (paging changed: the UI font and the panic strings are paged
     rodata, unclustered code moved into cluster tail lines):
     `cargo xtask dev soak mixed` (or the lead's usual soak) with Pictures
     and a large folder browsed: loads/s within `[soak]`
     (`loads_per_s_mean_max` 4,500; it was 2,670–3,220 during playback),
     0 underruns outside the windows, `pager_load_errors` 0.
  4. `cargo xtask dev cmd status` after browsing in the big folder: `pg`
     loads per second against the soak's figure while idle on a list.
### 2026-11-26 — A11Y-1: the screen reader on the host and in the simulator (worker, no hardware)

Features `orpheus-ui/screen-reader` and `orpheus-apps/screen-reader` (on in
the simulator, the browser build and xtask; **off in the firmware**: no
speech output there yet, TTS-3). Full "as built" in 05 §7.14; in short:

* **Toolkit** (`orpheus_ui::a11y`, `no_std`, no alloc): `Desc` (title,
  focus with role, label, value, state, unavailable, position "3 of 12",
  summary, toast, volume), `Screen::describe`, `Shell::describe` (dialogs
  and menu sheets over the screen), `Desc::list` (any `ListSource` +
  `ListView`), `Desc::problem` (the lint); `Reader` (new screen/overlay,
  moved focus, changed value after a key, toasts, volume; a key press
  stops speech first; verbosity Brief/Normal/Detailed; earcons enter,
  back, menu, dialog, wrap, end, on, off, toast, reader on/off; where am
  I, time and battery); `SpeechService` (stop, say, earcon, configure),
  `UtteranceQueue<N>` (byte ring), `Chords` (on raw edges before the
  mapper), `KeyMapper::cancel`, `KeyGate::reader_keys` / `forget`,
  `Config::timeouts`.
* **Apps**: `describe` on all 39 screen types (72 screens by name); the
  dispatch adds the strip hint to screens without a focus. Games say
  their state, not their boards; text and book pages their first 90
  bytes.
* **Settings › Accessibility** (after A11Y-2's rows): Screen reader
  `0x1004`, Speech rate `0x1005` (stored alone: quarters 1–40, "0.25×" to
  "10.0×", default 1×, the slider), Voice `0x1006`, Pitch `0x1007` (−4 to
  +4 semitones), Verbosity `0x1008`, Sound cues `0x1009`, Where am I
  `0x100A` (Off, Back + OK, Prev + Next), Keys with screen off `0x100B`;
  six packed in the word `0x0D61` (default 2896). `services::
  reader_config`. **Found**: `STORED` is a hand count — new stored items
  need it raised (`READER_STORED`), or their sets are dropped silently
  (caught by the rate test, not by a build error).
* **Deviation from 05 §7.14**: the toggle "hold Vol+ and Vol− 2 s" cannot
  be seen by the hardware (both keys on ladder 1, one key per ladder at a
  time, 05 §1). Built: **hold Back + Menu 2 s** (works from any screen,
  locked, screen off, reader off; no screen acts on those keys going
  down); also the Settings row. Where am I: Back + OK held for the hold
  delay, 2 s: time and battery. Owner decision wanted (user check 34).
* **Screen-off operation**: with the reader on and Keys with screen off,
  keys work with the screen dark (no waking press) and the backlight
  timeout does not lock the keys (default Auto-lock is "At once", which
  would have locked them); Power still locks: "Keys locked", "Unlocked".
* **Simulator**: `sim/src/speech.rs` (`SimSpeech`: the queue, `orpheus-tts`
  `Speaker` at 16 kHz in simulated time, earcons from `orpheus_synth::
  math::sin_turns`, a log with times, FNV-1a hash, WAV); `Sim::a11y`,
  `Host::reader_config`/`set_reader`; script `chord:a+b[:ms]`,
  `expect-said:`. Workspace profile: `orpheus-tts` and `orpheus-synth`
  at opt-level 3 in dev/test (a 22 s spoken journey: 7.8 s → 0.04 s).
* **Tests**: 9 `a11y` unit tests; `sim/tests/screen_reader.rs` — the lint
  (93 journeys, all 72 screens, 629 foci and menu items: title, label,
  something said at each verbosity; report `target/orpheus/a11y-lint.txt`),
  8 spoken goldens `sim/golden/speech/*.txt` (texts, earcons, stops with
  times, audio hash), behaviour tests, one differential test per setting;
  `settings.rs reader_packed`; matrix rows `s1004–s100b` and a new
  service context `speech` (Home with the reader on, two moves, where am
  I) — it showed real effects of Language, Home screen, Rotate and Hold
  delay there (declared). `settings-matrix --check`: 182 settings, 0
  errors. Matrix 199 rows pass in 10 s.
* **WAVs for the owner**: written by `ORPHEUS_SPEECH_WAVS=DIR cargo test
  -p orpheus-sim --test screen_reader spoken` (16 kHz mono, one per
  journey).
* **Firmware**: unchanged (the feature is off there): `cargo xtask build` resident
  81,876 B, 125 code + 28 rodata pages (as after SYN-3); `size --check`
  within budgets (resident 81,875 B, bss 124,296 B); `stack --check`
  margin 5,212 B. Reader RAM for later (host sizes): `Desc` 536 B,
  `Reader` 296 B, `Chords` 24 B, `UtteranceQueue<1024>` 1,048 B, plus the
  `Speaker` 3,352 B.
* **Gate**: `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -D warnings`, and `-p orpheus-ui` / `-p orpheus-apps`
  each with and without `screen-reader`; `cargo test --workspace` (2,099
  passed, 0 failed, 13 ignored); `cargo xtask settings-matrix --check`
  (182 settings, 12,922/12,969 cells, 0 errors); `cargo xtask build`,
  `size --check`, `stack --check`. Goldens: `app_group12_dark_latin`,
  `app_group12_light_cjk` changed (the Screen reader row on the first
  page, a scroll thumb; looked at). **Not run**: `xtask test all` beyond
  these, `brand`/`site --check`, every `xtask dev` step (no hardware).
* **Open**: speech volume, ducking/pausing music, key echo, track changes
  away from Now Playing, reduced motion's long names, games' boards, the
  Quick panel's adjust hint says Up/Down (it is Prev/Next); firmware
  wiring (05 §7.14 "For the firmware"); user check 34 (listen; the toggle
  chord).

### 2026-11-26 — MIDI nearly clean on the player (lead)

* Merged `e1a0dc7c` (budget follows the PCM ring; the main loop off the
  synth's cache lines; the USB supervisor without its 20 ms timer; rest
  margin 4 ms). `dev backup-verify` ok. On the player (`e1a0dc7c`):
  * Update path after the USB-supervisor change: two `dev update`s (B → A
    and A → B with the same build), each ≈ 40 s, `update: usb installed`,
    store intact; `dev reboot` back in ≈ 3 s.
  * `playdir /.orpheus/test/suite/midi` (Automatic, cable, test look):
    **one underrun event in the whole run** (`ub=synth:rest:5ms` near
    Etude's end; Hill Song `underruns=2`, Sound tour 0) — was 18/3/9
    before this change and 28/15/14 two rounds ago. `cut=` ≈ 80 % of
    `rests=` (was 50 %); `late=` 1.0–2.6 ms; `pg=` 0–6 %; pager loads
    during Etude 1,570/s (was 2,641/s); UI ≈ 5 loops/s, busy ≈ 6 %.
    Hill Song sheds at 288 (`lvl=3/3`, `vlim` 4–8); Sound tour mostly at
    full quality with the clock 192 ↔ 288; all three to the end; Base
    after.
* Open (worker's report): the wake-up path still shares two lines with
  per-piece code (≈ 4 line loads per wake-up); Now Playing's 33 ms tick is
  the largest thread-mode job.

### 2026-11-26 — SYN-3: thread mode's line loads; a budget that follows the PCM ring; the main loop off the synth's lines; no 20 ms USB timer (worker, no hardware)

The lead's run of `b694ac61` (Automatic, cable, test look on, `playdir
/.orpheus/test/suite/midi`): underruns "Etude" 18, "Hill Song" 3, "Sound
tour" 9 (`ub=synth:render:8–11ms`, `ub=synth:rest:5ms`), `cut=` ≈ 50 % of
`rests=`, `late=` 1.1–1.8 ms, `pg=` 4–11 %, `load=` 24–46 % at 288 MHz,
"Etude" and "Hill Song" shed at Max. Host model (the demo songs
themselves), `layout.json` of `b694ac61`, firmware build; **nothing run
on the player** (the brief). Full "as built" in 04 §9 ("As built (SYN-3:
thread mode's line loads …)").

* **Why**: the synth's per-piece set (`synth-render` + `synth-inner` +
  `play-pcm`, 31 lines) and its wake-up entry (line 18) fill all 32 lines
  of the code cache. Each line thread mode loads in a rest evicts one and
  the render loads it again after: 2 × 175 µs a line, half of it on the
  real-time executor. The UI's and the USB task's polls sit on lines
  10–0 (the per-piece and wake-up code), so most of those reloads happen
  before the render: the governor's `pg=` and `load=` never see them.
  With thread mode busy through half the rests (`cut=` 50 %), the
  executor's two thirds less the reloads fell below the render's need at
  a load the budget allowed. The old PC profile agrees: 53,403 line loads
  in 20.2 s (`meta.json`), ≈ 46 % of real time, against `pg=` ≈ 8 %.
* **Model** (`orpheus_synth::load::sim` with thread-mode jobs and their
  lines, `Job`; `orpheus-midi/tests/governor.rs` `play`): calibrated to
  the player's `cut=` ≈ 50 % (a UI tick of ≈ 2.2 ms and 9 lines every
  33 ms — the one free parameter), today's code gives "Etude" 41, "Hill
  Song" 0, "Sound tour" 41 underruns, load 25–39 %. The budget following
  the ring alone: 0/0/0 (ring ≥ 17.7 ms); with the main loop off the
  synth's lines and the USB timer gone too: 0/0/0, ring ≥ 52 ms, reloads
  240 → 140 ms a second. With up to twice the thread-mode load (UI tick
  3–4 ms over 12–14 lines, 10 % other real-time work, or the USB task
  as the heavy job) today's code underruns 53–804 times a song, all
  three changes none; the line fixes alone do not hold there, the ring's
  budget alone nearly does but sheds "Sound tour" 65–83 % of its frames
  (with all three 9–37 %). A larger rest margin alone changes little.
* **Changes**:
  1. `orpheus_synth::load::budget_pct` / `Governor::set_head`: the
     budget 55 % with ≥ 30 ms of audio queued, 30 % at ≤ 12 ms, linear
     between; below Max the clock goes up first (D-4), at Max it sheds;
     nothing back and the clock not down while the ring is short.
     Firmware: `synth::head_us` once a piece, to `plan_piece` and
     `pace_rest` (which no longer reads the ring itself).
  2. `atj2157_hal::executor::main_loop` (out of `MainExecutor::run`):
     `hot.toml` `synth-block` after the key tick — line 18 now holds the
     key tick, the loop (206 B) and `play_midi`'s entry and jump table
     (to +998 of 1,024). To fit: the sleep timed on the counter's low
     word, single-writer counters, the probe's time from the rest's end
     (`wake::REST_END_HI`, no 64-bit clock read), key edges sent out of
     line (`main.rs` `send_keys`), the time driver's `schedule_wake` out
     of line (resident by `atj2157_hal::time::*`, `try_borrow_mut`: no
     panic site; also shrinks the Ticker's poll 200 → 134 B and
     `rest_until` 340 → 274 B).
  3. `usbdev::supervisor`: no 20 ms timer. Reboots and an update's
     read-back are asked for inside `UsbDevice::run`'s poll in the same
     task, and the `select` polls the supervisor right after it
     (`asked()`); 20 ms only while an update reads back
     (`update::usb_busy`), else a 1 s backstop.
  4. `orpheus_power::pace::MARGIN_US` 2 → 4 ms (`late=` 1.8 ms + the
     reloads after a rest).
* **Numbers** (thumbv7em): resident 81,812 → 81,876 B (`size` 81,875;
  44 B left), .bss 124,288 → 124,296 B, code pages 125/128, rodata 28,
  `synth-inner` 8,102 → 8,150 B (8 lines), `synth-block` 5,782 → 5,926 B
  (6 lines), stack margin 5,212 B (unchanged); `stack --check`
  `synth-render` 18 of 18, with its `fits_with` **32 of 32** (was 31).
* **Tests**: `regress_synth_underruns_at_low_load` (`orpheus-midi`
  governor: today's code ≥ 10 underruns in "Etude" with `cut=` 40–70 %,
  the ring's budget 0 in all three, all fixes 0 with the ring ≥ 20 ms;
  fails with `budget_pct` a constant), `demo_songs_play_through_more_thread_mode`,
  `load::tests::the_budget_follows_the_ring`, xtask
  `regress_main_loop_on_the_render_lines` (the loop in `synth-block`
  after the tick, before `play_midi`). `regress_synth_rests_idled_while_the_ring_ran_dry`
  now runs with the old budget (`ring_budget = false`: the ring's budget
  alone sheds that model clean).
* **Gate**: host fmt, clippy `-D warnings` (all targets), `cargo test
  --workspace` 2,064 passed, 0 failed, 13 ignored, `settings-matrix
  --check` (173 settings, 0 errors); fw fmt, clippy, `xtask build` (125
  code + 28 rodata pages), `check-resident` (ok, none paged), `size
  --check` (within budgets), `stack --check` (margin 5,212 B). **Not
  run**: any `xtask dev` step (the brief), `brand`/`site --check`.
* **Open**: (1) the wake-up path still crosses per-piece lines:
  `play_midi`'s poll resumes `pieces` from line 22 (`Feeder::run`'s) and
  `player_request`'s poll (in no synth cluster) is on line 24
  (`render_i16`'s first) — ≈ 4 line loads a wake-up, ≈ 7 % of real time
  at 100 wake-ups a second, not in the model's figures; no room left
  (the synth's set is 32 of 32 lines): a layout that frees a line is the
  next lever. (2) Now Playing's 33 ms UI tick is the model's largest
  thread-mode job; a slower tick while a synth plays was not done.
  (3) Whether thread mode on the player is UI-heavy or USB-heavy is not
  known: check 7 below tells.
* **Lead's checks on the player** (Automatic, the cable in, test look on,
  Now Playing shown as in the last run): `cargo xtask dev backup-verify`
  (rule 2), `cargo xtask dev update`, then `cargo xtask dev cmd 'playdir
  /.orpheus/test/suite/midi'`, `cargo xtask dev cmd status` every ~9 s,
  the log:
  1. **Underruns: "Etude" 0, "Hill Song" 0, "Sound tour" 0** (≤ 2 at a
     song's very start acceptable); was 18/3/9. Any left: their `pcm:
     head` labels and the `syn:` line of that moment.
  2. `syn: cut=` ≥ 80 % of `rests=` in every song (model 91–92 %; was
     ≈ 50 %); `late=` under ≈ 1,000 µs (was 1.1–1.8 ms).
  3. `pg=` the same or lower (≈ 3–9 %); "Etude" `clk=288/288 lvl=3/3`
     with `vlim` 8–14 (was 6–10); "Hill Song" `lvl=` 0 for stretches;
     "Sound tour" `lvl=0` most of the time. `vlim` under 6 for long, or
     `lvl=3` through all of "Sound tour", means the ring runs short for
     another reason: send the `syn:` lines.
  4. All three to the end (`idle frames=2469600` after "Sound tour"), no
     reset, no `gave up`; keys and the screen respond while "Hill Song"
     plays (lyrics move); `status` replies at once.
  5. USB (the supervisor's new wait): `cargo xtask dev update` **a second
     time** with the same build — it must read back and switch (`update:
     usb installed`, the reboot into it, `dev cmd version` the same
     build), as fast as before (the read-back runs every 20 ms while
     busy); `cargo xtask dev reboot` reboots within ≈ 0.1 s of the
     request.
  6. After the songs `cargo xtask dev stats`: back at Base within ≈ 3 s,
     idle as before.
  7. `cargo xtask dev stats` twice, ≈ 10 s apart, while "Etude" plays:
     the pager's `loads` per second (the `7f26ae50` profile: 2,641 a
     second) clearly fewer; the UI's loops (≈ 30 a second on Now Playing)
     and busy µs per second — a busy share near 10 % or more makes the UI
     tick the next thing to slow while a synth plays (open item 2).

### 2026-11-26 — rests end when thread mode sleeps: fewer underruns (lead)

* Merged `b694ac61` (the soft I2C masks nothing; a rest ends when thread
  mode goes to sleep; `syn: cut= late=`). On the player (`b694ac61`,
  Automatic, cable, test look on), `playdir /.orpheus/test/suite/midi`:
  * underruns per song: Etude 18 (was 28), Hill Song 3 (was 15), Sound
    tour 9 (was 14+); all three to the end (`frames=2469600`), no reset,
    clock back at Base after.
  * `cut=` ≈ 50 % of `rests=` (the worker expected ≥ 80 %: thread mode is
    busy in half the rests); `late=` 1.1–1.8 ms (RT wake-up latency, fine);
    `pg=` 4–11 %; `load=` 24–46 % at 288 MHz; Etude and Hill Song still
    shed at Max (`lvl=3/3`, `vlim` 6–10).
* Open: the remaining underruns in Etude are RT time, not budget. Next
  candidates: the main executor's loop on the render's cache lines
  (≈ 3 % of real time, the worker's open item), a lower governor budget
  for heavy songs, and what thread mode does during half the rests.

### 2026-11-26 — SYN-3: a rest ends when thread mode sleeps; the soft I2C masks nothing (worker, no hardware)

The lead's run of `7f26ae50` (Automatic, cable in, `playdir
/.orpheus/test/suite/midi`): 20 underruns in ≈ 107 s, all in "Etude" and
"Hill Song", at `load=` 25–40 % with the clock at 288 MHz, labels
`ub=synth:render:5–10ms` (one `synth:rest:5ms`), `pg=` 5–16 %; the PC
profile `target/orpheus/prof/20261126-124550/` (main checkout). Code
reading, host model, firmware build; **nothing run on the player** (no
`xtask dev`: the brief). Full "as built" in 04 §9 ("As built (SYN-3: a
rest ends when thread mode sleeps …)").

* **Hypothesis 1, the soft I2C masks interrupts: no.** `i2c_soft`,
  `yoton_bsp::accel` and `sensor::sample` take no critical section; the
  bit delays are `delay::wait_us` on T0 with interrupts open (≈ 1 ms of
  bus a sample, 25 Hz, thread mode: the RT executor preempts it
  anywhere). Masked in the sensor: the batch push (µs) and once a second
  the detector over 25 samples inside `sensor::with` (µs, plus a line
  load or two if its code was evicted: ≤ ≈ 0.4 ms once a second; left as
  it is). The profile's ≈ 8 % for the sampler is mostly **aliasing**: one
  sample every 64 ms against its 40 ms ticker sees five phases of it (314
  samples in all: ±2 samples per 1 %). No other thread-mode critical
  section in the playback path is longer than a NOR transaction (256 B,
  ≈ 175 µs, `orpheus_pager::bus`) or a clock switch (< 1 ms, not during
  these songs: the clock stayed at 288).
* **Cause (hypothesis 2, in a different form)**: the pace rule's rests
  (4 ms after 8 ms of work) gave thread mode a third of real time
  whatever it did with it — the profile shows thread mode **asleep 20 %**
  (`MainExecutor::run`'s WFE) while the RT executor, at 59 % of samples,
  was at the rests' cap. The render is ≈ 35 % (`load=`); the rest of the
  RT time is paging, the wake-up path's line loads after each
  thread-mode slot (`play_midi`/`run_player`/`player_request`/task polls
  ≈ 14 % of samples, most likely samples deferred past a line load), the
  PCM writer and the looks. A heavy stretch therefore drew the ring down
  to the rest threshold (rest + piece + 2 ms ≈ 10 ms of audio), and a
  5–10 ms piece (paging) or a rest that resumed late (`rest:5ms`: 4 ms +
  the wake-up path's line loads) emptied it. Hypothesis 3 (the outer part
  every 4,096 frames) adds to the same RT time but is not a long stretch
  on its own (its chores are noted in the ledger and would label).
* **Fix**: `atj2157_hal::wake::rest_until` (the rest's poll, in
  `synth-inner` in place of `wake_at`) leaves the resting task's waker;
  the main executor, about to sleep during a rest, marks itself asleep and
  has the resting task woken 50 µs later (`wake::thread_sleeps`, inlined
  into its loop; `orpheus_power::pace::{rest_over, probe_due,
  IDLE_PROBE_US}`); if thread mode is still asleep then, the rest ends.
  The rest clears the mark as it begins and every wake-up of thread mode
  clears it, so a rest never ends while thread mode has work (up to 4 ms
  as before: the starvation and watchdog cases are unchanged). The main
  loop's WFE is the HAL's own `asm!` without `nomem` (the mark's stores
  stay on their side). To fit: `syn_set` was a CAS loop inlined eight
  times a piece; one writer, so a plain read-modify-write, out of line in
  `synth-inner` (`after_piece` 1,276 → 988 B).
* **Instrumentation** (`status` `syn:`): `rests=` (per file, now counted
  in the HAL), **`cut=`** (rests ended by thread mode's sleep), **`late=`**
  (most µs a rest's task resumed after its time since the last `status`:
  the RT executor's wake-up latency — masked stretches, higher levels,
  the wake-up path's line loads).
* **Model** (`orpheus_synth::load::sim`: `Setup::thread_pct`,
  `rest_ends_idle`, `Report::{max_backlog_us, rests, rest_us}`): "Etude"'s
  shape (Max, 9 voices), 15 % other RT work, 10 % paging, thread mode
  20 %: fixed rests underrun (≈ 370 frames in 30 s), rests that end on
  sleep (mean ≈ 0.3 ms) do not (also with 20 % other work); thread mode's
  work waits ≤ ≈ 2 ms (was 3.4) and keeps 20 % of every 100 ms.
* **Numbers** (thumbv7em): resident 81,812 B (build) / 81,811 B (`size`),
  unchanged; .bss 124,256 → 124,288 B (the rest's words, 28 B; the
  file's rest counts, 8 B); code pages 125/128; rodata 26 → 28 pages
  (+179 B of status text and the like regrouped one 16 KiB group; the
  synth's tables keep their offsets; slot 1,323,008 → 1,339,392 B);
  `synth-inner` 8,166 → 8,102 B (8 lines), `synth-block` 5,770 → 5,782
  B; `MainExecutor::run` 1,498 → 1,614 B (its loop still on lines 9–10);
  stack margin 5,476 → 5,212 B (.bss crossed a 256 B step); `stack
  --check` `synth-render` 18 of 18, with its `fits_with` 31 of 32.
* **Tests**: `regress_synth_rests_idled_while_the_ring_ran_dry`
  (`orpheus_synth::load`: the old rule underruns in the model, the new
  does not; fails if `pace::rest_len` goes back to a fixed 4 ms),
  `pace::tests::a_rest_ends_when_thread_mode_sleeps`; the governor,
  starvation, short-ring and overload tests unchanged and passing.
* **Gate**: host fmt, clippy `-D warnings` (all targets), `cargo test
  --workspace` 2,060 passed, 0 failed, 13 ignored, `settings-matrix --check` (173 settings, 0
  errors); fw fmt, clippy, `xtask build` (125 code + 28 rodata pages),
  `check-resident` (ok, none paged), `size --check` (within budgets),
  `stack --check` (margin 5,212 B). **Not run**: any `xtask dev` step
  (the brief), `brand`/`site --check`.
* **Open**: the main executor's loop is on physical lines 9–10
  (`render_i16`'s last, `synth-inner`'s first: `pieces`' poll and
  `plan_piece`): every thread-mode slot costs the render ≈ 2 line loads
  (≈ 0.35 ms, ≈ 80–100 slots a second in a heavy song, ≈ 3 % of real
  time). A loop function of its own clustered onto line 18 with the key
  tick would remove it (not done: it moves `synth-block`).
* **Lead's checks on the player** (Automatic, the cable in; `cargo xtask
  dev update`, then `cargo xtask dev cmd 'playdir
  /.orpheus/test/suite/midi'`, `cargo xtask dev cmd status` every ~9 s,
  the log):
  1. **Underruns**: 0 over the three songs (≤ 2 at a song's start
     acceptable); was 20. Labels, if any, in the `pcm: head` lines.
  2. **`syn:` `cut=`**: most of `rests=` in "Etude" and "Hill Song"
     (≥ 80 %: thread mode is idle long before 4 ms); `rests=` as before or
     fewer (the ring fills, and a wait for it counts as a rest).
  3. **`late=`**: a few hundred µs to ≈ 1,500 µs (the wake-up path's
     line loads). **≥ 5,000 µs** means something held the RT executor
     off for that long (a masked stretch or a fault storm): send the
     `status` lines and run `cargo xtask dev prof --faults 30 --during
     "cmd playdir /.orpheus/test/suite/midi"`.
  4. Same `clk=`/`lvl=`/`vlim=` as the last run; `pg=` the same or a
     little lower; all three songs to the end (`idle
     frames=2469600` after "Sound tour"); no reset, no `gave up`.
  5. Thread mode: keys and the screen respond while "Hill Song" plays;
     `status` replies at once; after the songs `dev stats` back at Base
     within ≈ 3 s, idle unchanged.
  6. If underruns remain with `cut=` ≈ `rests=` and `late=` small: the RT
     executor is short of time itself — the open item above (the main
     loop's lines) or a lower budget is next.

### 2026-11-26 — synth governor round 3 on the player (lead)

* Merged `82b61a1e` (each file starts at High unshed, pin re-evaluated per
  plan, net calm-time restore, rests wait for the ring, key tick off the
  render's lines). On the player (`7f26ae50`, Automatic, cable),
  `playdir /.orpheus/test/suite/midi`:
  * Etude: `clk=288/288`, `lvl=3/3`, `vlim` 6–9 (was 4–7), `pg=` 5–10.
  * Hill Song: **now raised to 288 before shedding** (`clk=288/288`),
    still `lvl=3/3` at Max with `vlim` 9–10 (was 7 at 144), `pg=` 12–16.
  * Sound tour: full quality (`lvl=0/0`, `vlim=16`, `fx=7`), the clock
    moving 144 ↔ 192 ↔ 288 (log `synth: clock …` at `lvl=0`).
  * All three to the end (`frames=2469600`); no reset; no `gave up`.
  * **Underruns 20 in ≈ 107 s** (was 18), all in Etude and Hill Song, at a
    `load=` of only 25–40 %: not the CPU budget. Labels `ub=synth:render:5–
    10ms` and one `ub=synth:rest:5ms`. Open: find what empties the ring at
    low load (piece length vs ring size, paging spikes up to 16 %, card
    reads, the outer part every 4,096 frames).

### 2026-11-26 — MIDI plays to the end on the player (lead)

* Merged the synth working-set fix (`4e1e9a49`). On the player
  (Automatic, cable): `playdir /.orpheus/test/suite/midi` — **all three
  songs played to their end** (Sound tour `frames=2469600`, exact), no
  `synth: gave up`, no reset; `pg=` 3–15 % (was ≈ 250 %); `ratio=` 76–92;
  the clock moves 144 ↔ 192 ↔ 288 (`synth: clock … ` log lines).
* Open (a worker fixes them): Hill Song ran **shed at 144 MHz**
  (`lvl=3/3`, `vlim=7`, `load` 30–43 %) — the shed state carries over from
  Etude and the clock came down without restoring quality (D-4: in
  Automatic, shed only at Max); 18 underruns over ≈ 107 s, several
  `ub=synth:rest:4ms` (a rest started with too little in the ring); `pg=`
  10–15 % at times against the model's 2–5 %.
* Idle with the cable: Base 48 MHz. The owner's own screen timers lock the
  player outside tests; `ui test-look on` frees it (as the tools do).
### 2026-11-26 — SYN-3: every file starts unshed at High; quality back before the clock comes down; rests wait for the ring; the key tick off the render's lines (worker, no hardware)

The lead's run of build `4e1e9a49` (Automatic, cable in, `playdir
/.orpheus/test/suite/midi`): every song to its end, no "gave up", no
reset, `pg=` 3–15 % (was ≈ 250) — but "Hill Song" sat at `lvl=3/3 vlim=7
clk=144/144` at `load=30–43%`, underruns labelled `ub=synth:rest:4ms`,
and paging above the model's 2–5 %. Host tests, a governor model and the
firmware build; **nothing run on the player** (no `xtask dev`: the
brief). Full "as built" in 04 §9 ("As built (SYN-3 on the player: shed
state, rests, paging …)").

* **Causes**:
  1. *Shed state / clock*: a new file's governor took its clock step from
     the clock it found (Max, left by "Etude") while `status` resets
     `clk=` to High at the open and sets it only on a change — "Hill
     Song" really ran at Max (the outer part re-applies the step as floor
     4) but showed `clk=144/144`, and a light file started at Max for
     nothing; a clock below the step at any plan (the clock governor
     lowering the leftover clock to the open's High floor during the
     scan) pinned the governor **for the whole file** (no raise again:
     after a file that left Turbo, a heavy song sheds at 192); and a step
     came back only after a whole second (three for an effect) in which
     **every piece** with the step undone stayed under 50 % — one burst
     restarted the stretch. In the governor model with the player's
     ratio (78 %) and paging (12 %), "Hill Song" at Max kept the reverb
     off 84 % of the song. (Not a carry-over of the shed itself: the
     governor is new per file.)
  2. *Rest underruns*: the pace rule's 4 ms rest started whatever the PCM
     ring held (after the outer part or a card read had drained it).
  3. *Paging*: the model left out thread mode between pieces. From
     `layout.json`: the key scan's 10 ms tick (input task's poll on lines
     31 and 0, the ticker's on 20, `_embassy_time_now` on 3 — three of
     `render_i16`'s lines) evicted render lines in ≈ 40 thread slots a
     second (≈ 2 % of real time); the step sampler (25 Hz, `steps-sample`
     on lines 4–7, the SVF/envelope/filter-design/drum lines, ≈ 1.3 %
     while counting is on); the UI's redraws (position, karaoke lyrics —
     "Hill Song" had the highest `pg=`) and the rodata cache unmeasured.
* **Fixes**:
  1. `orpheus_synth::load::Governor`: `new` starts at High at full
     quality whatever the clock (the first pieces raise it as needed);
     `set_clock` sets the pin anew before every plan, and decisions count
     the lower of the clock asked and the clock the CPU runs; below the
     top nothing stays shed (a pin that ended undoes everything); a step
     comes back when the smoothed `load` is under 50 % and the step fits
     the 55 % budget, calm counted net (a piece over takes back its own
     frames); the clock comes down only at full quality (as before).
  2. `orpheus_power::pace::rest_first` (firmware `pace_rest`, the model's
     loop): a due rest starts only if the ring holds the rest + the piece
     + 2 ms; else the piece renders first — at most 4 ms of render put off
     and only while the render keeps up with its rests (piece ≤ ⅔ of its
     audio), else it rests anyway (thread mode's share kept).
  3. `hot.toml` `synth-block`: the key tick (808 B) in front of
     `play_midi`'s poll, on physical line 18 with the wake-up's entry, off
     the render's lines (MP3: one line where it was four). `synth-note`
     moves 23–3 → 24–4.
* **Model** (`orpheus-midi` governor loop, clock free, true cost 78 %,
  700 µs paging a piece): "Etude" Max, mean voice limit 8.9 (old rule
  7.5); "Hill Song" 144 → 192 → 288 in its first 4 s, 16 voices, reverb
  on 40 % (16 %); "Sound tour" 90 % at full quality, clock 144–288, shed
  only near its end; no PCM half above 62 %.
* **Numbers** (thumbv7em): resident 81,812 B (build) / 81,811 B (`size`),
  unchanged; .bss 124,256 B, unchanged; code pages 125/128; `synth-inner`
  7,882 → 8,166 B (8 lines); `synth-block` 4,966 → 5,770 B (6 lines);
  stack margin 5,476 B; `stack --check` `synth-render` 18 of 18, 32 of 32
  with its `fits_with` (the check's padded count of `synth-inner` is 9
  lines; placed in 8). No slack left in the synth's 32 lines.
* **Tests**: `regress_synth_shed_carried_into_the_next_file`,
  `regress_synth_restore_held_off_by_note_bursts`,
  `regress_synth_rest_with_a_short_ring` (`orpheus_synth::load`; the
  first fails with the old `new`/pin, the second with the old restore,
  the third with `pace::due` alone: 4 short rests),
  `pace::tests::rest_first_waits_for_the_ring_within_a_bound`; the sim
  models the clock the last file left (`Setup::left`, `first_hz`), other
  real-time work (`Cost::stall_us`) and the ring rule. Changed:
  `restores_in_reverse_order` (4 voices + 1 fit the budget now: the limit
  comes back), `demo_songs_fit_the_budget_at_144_mhz` (pinned 144: single
  pieces over the budget ≤ 15 % of frames, was 3 % — "Hill Song" 13 %,
  still no PCM half at risk; full quality < 10 %, "Sound tour" 7 %).
  `regress_synth_render_starved_thread_mode`, the overload and Battery
  saver tests unchanged and passing.
* **Gate**: host fmt, clippy `-D warnings` (all targets), `cargo test
  --workspace` 2,058 passed, 0 failed, 13 ignored; `settings-matrix
  --check` (173 settings, 0 errors); fw fmt, clippy, `xtask build` (125
  code + 26 rodata pages), `check-resident` (ok, none paged), `size
  --check` (within budgets), `stack --check` (margin 5,476 B). **Not
  run**: any `xtask dev` step (the brief), `brand`/`site --check`.
* **Lead's checks on the player** (Automatic, the cable in; `cargo xtask
  dev cmd 'playdir /.orpheus/test/suite/midi'`, `cargo xtask dev cmd
  status` every ~9 s, the log):
  1. **Etude**: `clk=288/288` within the first second (log `synth: clock
     192 …` and/or `synth: clock 288 …` right after the open — new: every
     file starts at 144 and logs its raises), `lvl=3/3`, `fx=1`, `vlim=`
     mostly 7–12 (was 4–7), `load=` 35–50, `pg=` ≈ 6–9; underruns 0 or a
     few at its start, **no `ub=synth:rest:`** label.
  2. **Hill Song**: starts `clk=144`, log `synth: clock 192`, then `clock
     288` within ~4 s; then `clk=288/288`, `vlim=16`, `lvl=` 0 or 2 by
     turns (`lvl=…/2`, never 3), `fx=` 7 or 3; **not** `lvl=3 vlim=7`;
     underruns 0.
  3. **Sound tour**: `lvl=0` most of the time, `vlim=16`, `fx=7`, `clk=`
     moving 144/192/288 (log lines a few seconds apart), shed (`lvl=3`)
     only in its last seconds; underruns 0.
  4. End: `idle frames=2469600`, underruns total ≤ a few; after it `dev
     stats` back at Base within ≈ 3 s.
  5. `pg=` 6–9 % expected (2.4–3.3 render + ≈ 1.3 step sampler + UI).
     ≥ 10: run `cargo xtask dev prof --faults 30 --during "cmd playdir
     /.orpheus/test/suite/midi"` and send it (the UI's redraw lines or the
     rodata cache); compare `steps status` (counting on/off) with `pg=`.
  6. Battery saver (the owner's: user check): a MIDI file plays shed at
     the saver clock (`pin=1`), no stop.

### 2026-11-26 — SYN-3: the steady MIDI working set fits the code cache; a paused track releases its clock floor (worker, no hardware)

The lead's run of build `8240f128`: `ratio=` 100–108 and the clock at
288 MHz, but the pager's line loads were 200–250 % of real time during the
render (`pg=248`) and every demo song was given up. Firmware build, host
tests and an emulated model; **nothing run on the player** (no `xtask
dev`: the brief). Full "as built" in 04 §9 ("As built (SYN-3 paging …)").

* **Cause** (emulated, below): nine functions the render calls every
  32-sample segment were outside `synth-render` — the filter design,
  `exp2`, `log2`, the pan gains, six drum kinds — on physical lines
  **inside** it (`exp2` on line 22 with `render_i16`'s +5 KiB: 46 loads a
  piece on that line alone). `stack --check` passed (25 of 26 lines),
  but the render ran with `synth-midi`: 34 lines for 32.
* **Method** (as for AU-8 Opus): a bare-metal harness of the firmware's
  per-piece synth work (plan, render with the MIDI event loop, measure;
  thumbv7em, the firmware's profile, `render_i16` 12,408 B in both) plays
  the three demo songs whole in unicorn (Cortex-M4), every executed block
  recorded per piece, mapped by symbol + offset onto `layout.json` and
  replayed through a 32-line direct-mapped cache model; the firmware's own
  per-piece, per-wake and per-outer code counted as whole functions (the
  big polls on a wake: their first line and the resume call's line).
  Scratch tools, not committed. Old layout, "Etude": **96 line loads a
  piece, `pg=` ≈ 240 % render only, 290 % in all** — the player's order.
* **Changes**:
  * `hot.toml`: `synth-render` (18 lines) holds everything the render
    reaches every segment; `synth-rare` (rare drum kinds, the parser),
    `synth-note` (a note's start/end, the sequencer, a shed),
    `synth-inner` (8 lines: the per-piece code), `synth-block` (the outer
    loop), `synth-cold`; `play-block` split into `play-pcm` (5 lines, the
    part a synth shares: wake-up polls, half boundary, the per-block
    writer `Feeder::run` and `note_as`, before in no cluster) and
    `play-block` (10). Physical lines: `play-pcm` 19–23, `synth-render`
    24–9 (`starts_over` `play-block`), `synth-inner` 10–17,
    `synth-block` 18–22, `synth-rare` 17–23. Codec path 11 + 5 + 10 + 6 =
    32 (its two per-block conflicts, `Feeder::run` and `note_as` on
    `mp3-frame`'s lines, gone). `synth-midi` is gone.
  * Firmware loop: `pieces` (own poll) runs pieces until 4,096 frames
    (93 ms) are written or `attention` (stop, player/card request, the
    player's actions, fade to pause, a log due); the outer part (requests,
    pause/seek, the floor re-applied — was every piece —, position,
    tick, chores, logs) runs then. `render_piece` (the render call and its
    timing). The rest waits on `RestUntil` (HAL clock +
    `atj2157_hal::wake::wake_at`) instead of `embassy_time::Timer`, whose
    poll and `_embassy_time_now` sat on the per-piece and render lines.
    A background job gets a chore step every 93 ms while MIDI plays.
  * Host (no arithmetic change): `Engine::estimate`, `Engine::shed`,
    `Governor::{over_budget, restore, lower}` out of line, `MidiPlayer::
    timed` split (`parse_next` out of line). `plan_piece` 2,856 → 1,300 B,
    `play_midi`'s poll 7,146 → 4,966 B.
  * `xtask stack --check`: a cluster's reachable code plus its `fits_with`
    clusters must fit 32 lines (07 §5.6); `synth-render` 18 of budget 18,
    with `synth-inner` and `play-pcm` 31 of 32.
* **Model, the three demo songs whole at 288 MHz** ("Etude" / "Hill Song"
  / "Sound tour"): line loads a piece 2.3 / 2.3 / 1.8; **`pg=` (render
  only, as `status` shows it) 3.2 / 3.3 / 2.4 %**; with the rest of the
  loop 6.9 / 7.0 / 5.3 %. Not modelled: thread mode between pieces (the
  UI), the step sampler, the rodata cache.
* **Paused track** (lead 2026-11-26: Mid 96 MHz while paused):
  `orpheus_power::governor::audio_floor` / `floor_level` (host): no floor
  while paused, the governor treats it as nothing playing (Low; Base on
  the cable); `pause` sets `audio::PAUSED` and releases the floor,
  `resumed` restores it at once. The picture boost is its own flag (its
  end no longer clears a playing track's codec floor). Tests
  `regress_paused_track_keeps_clock_floor`, `audio_floor_parts`.
* **Numbers** (thumbv7em, `xtask build`/`size`/`stack`): resident 81,652
  → **81,812 B** (+160: the time queue's `schedule_wake`, now out of line
  and resident by name, 104 B; the executor's
  `__embassy_time_queue_item_from_waker`, 60 B, added to `resident.toml`'s
  timer-queue rule, which its unmangled name had escaped); .bss 124,240 →
  124,256 B; code pages 125/128 (1,012,996 B paged); stack margin 5,476 B
  (unchanged); clusters `synth-render` 18,360 B, `synth-inner` 7,882 B,
  `play-pcm` 5,010 B, `play-block` 10,136 B, `synth-rare` 6,420 B,
  `synth-note` 12,450 B, `synth-block` 4,966 B, `synth-cold` 5,744 B.
* **Tests**: `regress_synth_working_set_over_the_cache` (25 + 9 = 34
  fails, 18 + 13 passes; fails without the check),
  `working_set_with_its_fits_with_fits_the_cache`,
  `hot_toml_synth_is_checked_and_placed`,
  `regress_paused_track_keeps_clock_floor`, `audio_floor_parts`; the
  engine, MIDI and governor goldens unchanged (bit for bit).
* **Gate**: host fmt, clippy `-D warnings` (all targets), `cargo test
  --workspace` 2,054 passed, 0 failed, 13 ignored; `settings-matrix
  --check` (173 settings, 0 errors); fw fmt, clippy, `xtask build` (125
  code + 26 rodata pages, no `fits_with` warnings), `check-resident`,
  `size --check`, `stack --check` (margin 5,476 B; `synth-render` 18 of
  18, 31 of 32 cache lines; `games-io` 14 of 14, 31 of 32). **Not run**:
  any `xtask dev` step (the brief), `brand`/`site --check`.
* **Lead's checks on the player** (governor on, Power mode Automatic,
  the cable in):
  1. `cargo xtask dev cmd 'playdir /.orpheus/test/suite/midi'`, then
     `cargo xtask dev cmd status` every 10 s through all three songs:
     **`syn: pg=` 2–5** (≥ 10 is a finding: report it with the fault
     profile below), `ratio=` 95–115, `clk=` 288 for "Etude" (it needs
     Max unshed), lower stretches (192/144) in "Hill Song" and "Sound
     tour"; `lvl=` ≤ 2 most of the time with `vlim=` 16 for the two
     lighter songs; `dec:` loads a render piece ≈ 1–3 (was ≈ 80); every
     song to its end (frames as `orpheus-midi/tests/goldens-play.txt`),
     no `synth: gave up`, `underruns` 0 or a burst at a song's start; UI
     alive (open Files while it plays: a listing arrives within ~0.1 s).
  2. Fault profile while it plays: `cargo xtask dev prof --faults 30
     --during "cmd playdir /.orpheus/test/suite/midi"` — expect few
     faults in `synth-render` and `synth-inner`; most in `synth-note`,
     `synth-rare`, `synth-block`, the UI. Many in `synth-render` or
     `synth-inner` after the first second mean a line is shared that the
     model did not see: send the report.
  3. Paused floor: an MP3 with a Sound setting on (EQ), `dev stats` → level
     2 (Mid) while it plays; pause in Now Playing → within 3 s level 1
     (Base: the cable); play → level 2 at once, 0 underruns. Key click on:
     a key press while paused still clicks.
  4. Regression: Pictures while a Vorbis file plays (level 3 High kept
     after the picture closes), a games save during MP3 (`serve: games`
     pieces as before, `pl=` not higher).

### 2026-11-26 — PWR-5 and the synth governor merged; on the player: USB needs ≥ 48 MHz, MIDI is paging-bound (lead)

* Merged the synth measurement fix (`d9800bb8`) and PWR-5 (`ad7d5f85`:
  Performance removed, Power mode per the owner, the governor reaches
  288 MHz by itself, VDD clamped at `0x18`). Conflicts in `governor.rs`,
  `audio.rs`, `main.rs` and the MIDI governor test resolved hunk by hunk
  with PWR-5's side (its floor-as-level design covers the synth's floors
  3/4); a first attempt that took whole files was undone before it was
  pushed. Gate: 2,049 host tests, matrix 0 errors, fw build/size/resident,
  stack margin 5,476 B. Installed `8240f128`; store unchanged.
* **U-PWR-5 answered on the player: USB does not work at 24 MHz.**
  `clock 24` with the governor off: the controller looped in
  `usb: recovery (recover-setup)` (12 resets), then `usb: wedged … watchdog
  not fed` and the watchdog restarted the player (~30 s). PWR-5's
  `PERIPHERAL_MIN[usb] = Base` (48 MHz) is right; keep it.
* Idle with the cable and no track: Base 48 MHz (as designed). With a
  track loaded but **paused** (resume at start-up): Mid 96 — a paused track
  keeps its clock floor. Open: drop the floor while paused.
* **MIDI on the player** (`playdir /.orpheus/test/suite/midi`): the
  measurement is right now (`ratio=` 100–108, the clock rises to
  `clk=288/288`), but **paging is 200–250 % of real time** (`pg=248`): the
  synth's working set (`synth-render` 25 lines + `synth-midi` 9 lines) is
  larger than the 32-line code cache, so every piece reloads code. All
  three songs ended with `synth: gave up, nothing can play at 288
  load=261% ratio=108 pg=248% lvl=3 vlim=2`; no reset, UI alive. The
  `play PATH` command named in the SYN-3 checks does not exist in this
  build (use `playdir`).

### 2026-11-26 — PWR-5: Performance removed; the governor reaches 288 MHz by itself; Power mode as the owner defined it (worker, no hardware)

D-4 as revised by the owner ("Dynamically clock up to 288mhz if required.
Remove the manual 'performance' setting …") and the owner's Power mode
definitions of the same day. Nothing run on the player.

* **Removed**: Settings › System › Performance (`0x0C07`) — the setting,
  its three strings (en/ru/ja), `id::PERFORMANCE`, `governor::Performance`
  and the governor's `perf` input, `drain::Shared::perf_max` and the UI's
  store into it, the drain log header's `perf=` field (`xtask drain report`
  still reads it from older files; the table's column is now the label),
  the soak's `Perf` pick, the session whitelist entry, the matrix row
  `s0c07`, the manifest entry, 12 goldens and their doc screenshot,
  `docs/user/performance.md` (now `docs/user/power.md`, Power mode). The
  key is **retired** (`settings::RETIRED`): never reused, an old store's
  record is skipped; store keys in use stay 131 of 192 (the stored tree is
  one smaller, the retired list one longer).
* **Freed** (firmware, `cargo xtask size` and `llvm-nm` against
  `e3866cc4` built the same way): what the removal took out (the
  Performance strings −24 B rodata, the drain header's `perf=` −80 B code,
  the UI's per-loop publication, the old `Governor::step` 166 B) is spent
  again by the new policy (power model, peripheral minimums, floors to
  Max, `raise_to_floor` 60 B; the watchdog task's poll, where the step is
  inlined, +336 B). Overall: paged code **+40 B** (1,010,038 → 1,010,078 B
  in sections, still 125/128 pages), paged rodata +40 B (26/48 pages),
  resident +8 B (81,623 → 81,631 B), bss unchanged (124,232 B), SRAM3
  `drain::Shared` one byte less. Stack: 14,492 B nested worst case, margin
  5,476 B. `dsp_clock_floor` is out of line (inlined into its callers it
  cost 44 B more).
* **Governor** (`orpheus_power::governor`, 02 §5.4, 03 §2.4):
  * **Automatic**: never below the audio floor (`CODEC_FLOOR` /
    `CLOCK_FLOOR` now 0 Base, 1 Mid, 2 High, 3 Turbo, 4 Max — the old
    values unchanged; nothing playing and 0 = no floor) or the peripherals'
    minimum. Saturated (idle < 25 %) → `climb`: Low → Base → Mid → Turbo,
    Max only from a saturated Turbo. Otherwise `steady`: the level of
    least modelled power (`COST`, estimates, U-PWR-6) that carries the
    measured demand (busy % × MHz; the current level up to 75 % full,
    another up to 60 %); down after 2 calm steps to the highest level asked
    in them. Idle → Low (24 MHz, PLL off) — new: the old governor never
    went below Base.
  * **Battery saver**: pinned at `peripheral_min(active)` (USB `Base`
    while VBUS is present, until U-PWR-5; card, display, audio, pager,
    keys `Low`, each with its source in `PERIPHERAL_MIN`'s doc); audio
    floors and the vault's key derivation do not raise it
    (`raise_for_floor`).
  * **High performance**: pinned at Max (288 MHz, 1.3 V), screen on or off.
  * Under a host test (test look, session, mailbox) the governor follows
    Automatic (`PowerMode::for_governor`); the test look also reads Power
    mode as Automatic (marquees).
* **Supply**: `orpheus_power::clock` now holds the levels (the HAL
  re-exports `Level`), their supply codes and `switch` (supply before the
  clock going up, after it going down: the existing HAL order, now
  host-tested over every pair) and `vdd_write`, the HAL's one writer of
  `VOUT_CTL[4:0]`: clamped to `VDD_MAX` = `0x18` (1.3 V; [DS] §2.1 absolute
  max 1.35 V, §2.2 recommended 0.9–1.35 V) with a const assertion that no
  level maps above it. 01 §7 and §9 (U-PWR-5, U-PWR-6) updated.
* **Floor API for the synth worker** (stable, extended): store 0…4 into
  `audio::CODEC_FLOOR` and call `dsp_clock_floor(..)` as before; 3 = Turbo
  and 4 = Max now take effect (raised at once in Automatic through
  `crate::raise_to_floor`, held by the governor; ignored in Battery saver,
  already there in High performance). `crate::CLOCK_FLOOR` keeps the same
  encoding.
* **SPI0 / paging at Low** (lead's item 2): not changed. At `Low` the HAL
  already puts SPI0 on CK24M (`0x100`, 24 Mbit/s); at the PLL levels it
  runs from COREPLL at 48 MHz. So page loads take about twice as long at
  24 MHz (≈ 340 µs per 1 KiB line). Moving SPI0 to DEVPLL would make it
  independent of the level but keep DEVPLL running (vendor: 180 MHz,
  always on), which costs power in every mode and needs U-PWR-3 settled;
  not cheap, not done.
* **Tests** (host): `clock::tests::switch_order_keeps_the_clock_within_the_supply`,
  `no_supply_above_1_3_v`, `levels`; `governor::tests::saturated_work_climbs_and_comes_back`,
  `max_only_for_sustained_demand_above_turbo`, `steady_demand_runs_at_the_cheapest_level`,
  `floor_holds_up_to_max`, `no_max_with_the_screen_off_and_nothing_demanding_it`,
  `peripheral_minimum`, `power_mode_setting_values` (the 0x0401 differential
  test), `regress_tests_follow_automatic_whatever_the_owner_chose`;
  `sim/tests/apps.rs::system_has_no_performance_row`.
* **Gate**: host fmt, clippy `-D warnings`, `cargo test --workspace`,
  `settings-matrix --check` (0 errors), `site --check`; fw fmt, clippy,
  `cargo xtask build`, `check-resident`, `size --check`, `stack --check`:
  all pass. `dev hil smoke` **not run** (no hardware in this session).
  Goldens re-blessed and looked at: System's list and the dialogs over it
  differ only in the scroll thumb (6 rows instead of 7); the suite's
  expected `settings/system.png` likewise.

**Lead's checks on the player** (`xtask dev stats` prints `clock level N
(M switches)`: 0 Low 24, 1 Base 48, 2 Mid 96, 3 High 144, 4 Turbo 192,
5 Max 288 MHz). The developer channel itself runs under the test rules
(Automatic), and with the cable in Automatic never goes below Base:

1. **U-PWR-5 first** (USB at 24 MHz): `dev cmd 'gov off'`, `dev cmd
   'clock 24'`, then `dev stats` (level 0), a screenshot, a file-transfer
   read of 1 MiB; `dev cmd 'clock 48'`, `dev cmd 'gov on'`. If any fails,
   leave `PERIPHERAL_MIN[USB]` at Base (the player would otherwise restart
   on the cable in Battery saver); if all pass it can become `Low`.
2. **Idle** (screen on, nothing playing, cable in): `stats` twice 10 s
   apart: level 1 (Base, the USB minimum), switches not rising.
3. **MP3** (`playdir` an MP3 folder): level 1–2 after the first seconds,
   switches settle (no flapping: under ~1 a second on average after the
   first 5 s), 0 underruns.
4. **MIDI** (a demo song): level 3 (High, floor 2) or higher if the synth
   worker's floors ask; with a heavy song the synth should reach 5 (Max)
   before shedding. `status` `syn:` shed count 0 at Max where it was > 0
   at 144 MHz.
5. **Busy UI** (scroll a long list continuously for 10 s): the level rises
   step by step (1 → 2 → 4 → 5 only if still saturated at 192 MHz), then
   falls within ~2 s of stopping.
6. **Supply order**: `dev ram clocks --test clocks` (the 2026-11-24
   level self-test; it now runs through the shared `switch`/`vdd_write`)
   must still PASS with the VDD read-back `0x0C` at 24–144, `0x10` at
   192 and `0x18` at 288 MHz.
7. **Power modes** need the owner (not in the session's whitelist): user
   check 19 and the Power mode lines of the Display/Power user check.

### 2026-11-26 — restart saves the place; synth governor on the player (lead)

* Owner 2026-11-26: "Reopen the track paused after a reboot". It already
  opened paused (`status`'s first word says `playing` for a loaded track;
  frames stay at 0), but the power menu's Restart and the developer
  channel's normal reboot rebooted without the card task's shutdown, so it
  reopened at the last saved place. Fix (`3f8d7803`): both go through
  `Ui::save_for_shutdown` (recording, game, battery log, the player's
  place) like power-off; the USB task reboots itself if that has not
  happened in 3 s. On the player: playing at 15.0 s → `dev reboot` → back
  **paused at 17.4 s** (frames 0), log `ui: restart`, `audio: resuming
  paused`. (Code window was 30 B over: the USB branch shares the reboot
  code.)
* Merged the synth governor (`1cdb7181`, `78b316b5`): bounded pieces with
  rests, shedding order chorus → reverb → voices, labelled underruns; code
  pages 125/128 (SHA-2 compact rounds). On the player: **no reset** in
  3 min of MIDI and the UI alive (the watchdog finding is fixed), underruns
  labelled `ub=synth:render:…`; `synth bench 32/29` 1,408/750 (reverb ≈ 658).
  But **every song stops after ~200 ms**: `ratio=400` (the clamp), `lvl=3/3`,
  `vlim=2`, then the "too slow" stop — at High 144 MHz, where the bench
  matches the table, so the playback-time ratio is mis-measured. A worker
  fixes the measurement and the stop rule. The resume point is an MP3.
### 2026-11-26 — SYN-3: governor ratio measured right; the clock goes up before anything is shed; underruns no longer end a song (worker, no hardware)

The lead's run of build `78b316b5`: no reset over 3 minutes, UI alive,
underruns labelled `synth:render:8ms` (7, 17), `synth bench` 32 / 29 =
1,408 / 750 (the table agrees) — but every song stopped at once:
`Etude.mid` idle within 3 s with `syn: v=9 vlim=2 over=57 lvl=3/3 fx=1
load=98 ratio=400 rests=56 shed=7`, the playdir within 20 s at
`underruns=16`. Firmware build and host tests; **nothing run on the
player** (no `xtask dev`: the brief). Owner direction D-4 (2026-11-26,
via the lead): clock up to 288 MHz when needed, shed only at Max, back
down when the music allows; Power modes (Battery saver: the lowest clock
the peripherals work at, degrade but play; Auto: everything at the least
battery; High performance: Max). Full "as built" in 04 §9 ("As built
(SYN-3 governor measurement and clock …)"); in short:

* **Cause** (code and a host model): (1) the ratio's **first measurement
  was taken as it was**, and the first piece of a song is estimated with
  the **empty engine** (245 cycles a frame) while its first notes start
  inside it (plus cold code and patches): ≥ 10 ×, clamped to 4.0 at once;
  the model reproduces it for all three songs (ratio 400 after piece 1,
  voice limit 2 at piece 2) even without a cold factor. (2) The render's
  time included the **pager's line loads**, a cost per piece, not per
  frame: as the ratio rose the pieces shortened and paging per frame grew
  — likely why the player stayed at 4.0 for seconds with two voices
  (renders of 7–17 ms); not proven without hardware, the new `pg=` shows
  it. (3) The codecs' **too-slow stop** (8 lost PCM halves over the whole
  file, no time window) ended the songs. Units (µs × MHz) were right.
* **Measurement** (`orpheus_synth::load::Governor::measured`): compute
  only (the pager's `LOAD_US` during the render is subtracted and kept as
  a smoothed paging figure the decisions count as fixed); the estimate is
  the mean of the engine's before and after the piece; nothing measured
  for 250 ms after an open or seek (`restart`), nor pieces < 32 frames or
  with an estimate < 2 × the empty engine's; falls by ¼ of the difference
  a piece, **rises ≤ 3 % per 256-frame block**. Piece length (only)
  follows the last piece's raw ratio from the first piece on.
* **Clock (D-4)**: over budget → the lowest of High/Turbo/Max (144 / 192
  / 288 MHz) that fits, at once from the RT task (clock floor 2/3/4,
  `audio::synth::synth_floor`, re-applied every piece); shedding only at
  Max; restore voices (as many as fit 50 % at once), effects, then the
  clock one step down after 2 s at full quality under 50 % of the lower
  clock. **`orpheus_power::governor` (minimal edit)**: floors 3 and 4 are
  honoured (down-step never below them; Max → Turbo for a Turbo floor;
  the Normal-performance "Max → High" rule skips a Max floor and goes to
  Turbo for a Turbo floor). The synth relies on: the clock governor never
  stepping below `CLOCK_FLOOR`, stepping down to it when idle, and the
  power-mode pinning the other worker builds; it reads the mode from
  `ui::POWER_MODE` only to ask no more than High in Battery saver.
* **Power modes**: when the CPU runs slower than the step asked for, the
  governor marks the clock **pinned** for the file and sheds at the clock
  it has (`syn: pin=1`); High performance gets Max's budget at once.
* **Giving up**: underruns no longer end a MIDI file. Only when everything
  is shed at the top (or pinned) clock and the measured render stays
  above 200 % of real time for 5 s of its own time (net): logged `synth:
  gave up, nothing can play at N load=… ratio=… pg=… lvl= vlim=`.
* **Logs / `status`**: `synth: clock N load=L% ratio=R pg=P% lvl= vlim=`
  at each clock change; `syn:` adds `clk=now/max` (MHz asked; set at a
  change, so 144/144 until the first), `pin=`, `pg=` (paging % of real
  time).
* **Model results** (`orpheus-midi` `tests/governor.rs`, true cost = the
  engine's state through each piece, measured as the player does), clock
  free: Etude at Max all the way, reverb off, mean voice limit 8.0 (=
  fixed 288 MHz); Hill Song 16 voices, reverb 17 %, Max 91 % / Turbo 7 % /
  High 2 %; Sound tour full quality 91 %, High 9 % / Turbo 58 % / Max 33 %;
  ratio 99–103 %, < 1 % of frames over budget past 2 s, no PCM half at
  risk, none given up. With the cold start and 350 µs paging a piece: the
  same frames, ratio ≤ 104 % past the first second. Pinned at 144 MHz
  (e.g. Battery saver at High): as before (Etude 2.4 voices, Hill Song
  4.2, Sound tour 6.1). A light song stays at High, bit for bit.
* **Numbers** (thumbv7em): resident **81,639 B** (`xtask size`; build
  line 81,644; was 81,623), .bss **124,240 B** (+8: the fourth `syn:`
  word), governor 40 → 60 B (MIDI state ≈ 71,960 B, computed),
  **125/128 code pages**, `synth-midi` 9 lines (9,058 B),
  `play_midi`'s poll 7,134 B in its 7-line hole (the give-up moved into
  `after_piece`), `synth-render` 25 of 26 lines, **stack margin 5,476 B**
  (unchanged).
* **Tests**: `regress_synth_ratio_pinned_by_cold_start` (two: the loop
  model in `orpheus_synth::load` — first piece estimated empty, 50 ms at
  10 ×, a 2 s burst at 3 ×: not stopped, ratio back to 95–105 %, the
  clock up for the burst and back to High, ratio ≤ 110 % without the
  burst; fails with the old measurement — and the three demo songs in
  `orpheus-midi`); `regress_synth_render_starved_thread_mode` kept (clock
  pinned and free; "sustainable after" bound 12 → 80 pieces, ≈ 0.25 s:
  the rise cap makes a real 1.3 × misestimate show more slowly; thread
  mode ≥ 30 % of every 100 ms, ≥ 40 % overall, no underrun);
  `raises_the_clock_before_shedding`, `restores_then_lowers_the_clock`,
  `battery_saver_degrades_and_never_stops` (24, 48, 96 MHz),
  `gives_up_only_when_nothing_can_play`, the unfixable overload plays on;
  `demo_songs_raise_the_clock_before_shedding`,
  `a_light_song_stays_at_high`; `orpheus-power` `floor_holds` (floors 3
  and 4).
* **Gate**: fmt (host, fw); clippy `-D warnings` (host workspace all
  targets, fw workspace); `cargo test --workspace` 2,048 passed, 0 failed, 13 ignored;
  `settings-matrix --check` (174 settings, 0 errors); fw `xtask build` (ok, 125 code +
  26 rodata pages), `check-resident` (ok, none paged), `size --check`
  (within budgets), `stack --check` (margin 5,476 B). Not run: any `xtask
  dev` step (the brief), `brand`/`site --check`, fuzzing beyond the
  corpus replay.
* **For the lead on the player** (Power mode Automatic; Performance
  Normal, or whatever the clock worker's build leaves):
  1. `play /.orpheus/test/suite/midi/Etude.mid`, `status` every ~10 s:
     plays to its end (`frames=` 1131268 at the end, or within a few
     hundred while playing out); `syn: clk=288/288 pin=0`, `ratio=` 90–
     115 after the first second (≫ 100 now means real compute the table
     misses, not paging), `lvl=3/3` with `vlim=` mostly 6–10 and `fx=1`
     (reverb off), `pg=` the paging share (expect a few %; ≥ 20 means
     paging is the problem: report it), `underruns` 0 or a burst at the
     start; log `synth: clock 288 …` once near the start; no reset, UI
     alive (`stats` `clock level Max 288` while it plays).
  2. `Hill Song.kar`: to its end (1146600 frames), `clk=` 288 most of the
     time with drops to 192/144 in quiet stretches (log lines `synth:
     clock …` a few seconds apart at most), `vlim=16`, `lvl=` max 2
     (reverb off at times), underruns 0.
  3. `Sound tour.mid`: to its end (2469600), `lvl=0` most of the time,
     `clk=` moving between 144, 192 and 288 (`clk=…/288`), underruns 0.
  4. `playdir /.orpheus/test/suite/midi`: all three in turn, no stop, no
     reset; after it `stats` back at High or lower (the clock governor
     steps down within ≈ 3 s of the end).
  5. Battery saver (Settings › Power › Power mode): the same Etude plays
     to its end, shed to the floor (`pin=1`, `vlim=2`, `clk=` the asked
     step, `stats` the saver clock), underruns allowed, no stop unless a
     `synth: gave up` line says why; UI alive.
  6. If a song still stops: the `synth: gave up …` line (or its absence:
     then something else ended it) and `status` at that moment.
* Open: whether paging is the player's 4 ×: `pg=` answers it; if it is
  large, the per-piece paging (lines `play-block` and the chores evict)
  is the next work, not the governor.

### 2026-11-26 — NOR read race fixed; 20 boots clean (lead)

* Merged `b7a4cb04` (thread-mode NOR reads masked against the pager,
  read errors by kind, `nor=C/D/I` in the session reply, the suite fails
  on any). On the player: **20 normal reboots, 10 of them with an MP3
  playing (the resume mounts the card at boot): every one `store=ok
  keys=25 crc=acff8170 nor=0/0/0`, no `settings: store` log line**; the
  owner's settings unchanged.
* New `cargo xtask dev reboot` (normal reboot through the developer
  channel). A loop with `dev reset` (through ymon) left the player in
  recovery mode on its 3rd boot for 30 min (it came back with `dev
  reset` from ADFU): use `dev reboot` for reboot loops.
* Observed: after a normal reboot during playback the player comes back
  *playing* (`status` playing), although Resume at start-up is described
  as opening paused (VER-13 round 2). Open: check which is intended.

### 2026-11-26 — HW: the boot-time settings read error is SPI0 shared with the pager; thread-mode NOR reads masked (worker, no hardware)

* **Cause (from the code, not yet seen on the player)**: `nor::read` (the
  settings store's, the vault's) ran with interrupts open, while the RT
  executor's task bodies are paged by design (resident.toml) and at boot
  the audio task mounts the card in parallel with the UI task's
  `mount_store`. A page fault there runs the pager's line read
  (`nor::read_dma`) on the same SPI0 in the middle of the thread's
  transaction: CS toggles (the chip drops the READ), `BC` is rewritten and
  `CTL &= 0x7FFF_EFBC` clears the mode bits. Cut in the data phase, the RX
  FIFO stays empty and `recv` times out (the error we saw); cut between
  command and data, bytes are clocked with no command and come back as
  0xFF **with no error** — `get` reads values without their CRC and a
  scan reading a bad header stops early, so settings could also have read
  wrong silently. An RT task's `clock::set_level` (the audio clock floor)
  switched SPI0's clock under such a read as the same race. Why some boots
  only: it needs an RT fault inside one of the mount's short reads; boots
  that resume playback or after an update (cold pages) fault the most. The
  installer's reads already masked themselves for exactly this reason;
  nothing else did. Ruled out: a NOR program/erase still running at boot
  (the driver cannot see chip busy on a read: it would return garbage, not
  an error), the JEDEC/mode set-up (only at `nor::init`, before any task),
  `Refused` (deterministic).
* **Fix**: `orpheus_pager::bus::read` (host crate, resident): every
  thread-mode read is a series of transactions of at most 256 bytes, each
  inside `critical_section` (BASEPRI, levels 1–3; faults stay live and
  only the reader's own resident code can fault then). An epoch counter
  bumped by `read_dma` and `set_clock` turns a transaction another SPI0
  user ran during into `Error::Read(Interleaved)` instead of data.
  `nor::Error::Timeout` for reads is now `Error::Read(Command | Data |
  Interleaved)`. `recv` stops at the first empty FIFO (it polled 200,000
  times per remaining byte: up to ~1 s per 64-byte piece with the main
  executor blocked). The installer's own masking is gone (the HAL does
  it). Masked time per transaction: a 256 B byte-polled READ ≈ one pager
  line read (~175 µs at `0x200`); the store's reads are 12–64 B.
* **Instrumentation**: `settings: store mounted after N read errors, last
  nor read/data` (or `…/command`, `…/cut`, `nor timeout`, …; also on
  `store not mounted`); `Store::mount_retrying` returns the last error.
  Counters `nor::read_errors()` (command, data, cut) in the `settings
  test-session` reply as ` nor=C/D/I`; `dev suite` fails when any is
  non-zero (`regress_suite_sees_a_hidden_nor_read_error`).
* **Tests**: `regress_store_read_cut_by_a_page_fault` (a model of the
  controller takes the pager's line read after every register step of a
  two-transaction read: right bytes and the pager served; fails without
  the mask), `an_unmaskable_cut_is_reported_not_returned`,
  `transactions_are_masked_chunks_in_order`,
  `a_failed_transaction_ends_the_read`; `regress_mount_survives_a_read_error`
  extended (last error, count).
* **Numbers**: resident 81,388 → 81,583 B (of 81,920); paged code
  1,047,958 → 1,048,246 B in the 128-page window (330 B left); `.bss`
  124,200 → 124,216 B; stack margin 5,440 B unchanged. A first try with a
  separate `nor errors` command overflowed the code window (129 pages);
  the counters went into the session reply instead.
* **On-device check (lead)**: after `dev update`, `cargo xtask dev cmd
  "settings test-session"` → `… nor=0/0/0`. Then 20 boots (`dev reboot`),
  half with a track playing before the reboot (resume at start-up mounts
  the card in parallel), each followed by the same command and a log
  check for `settings: store`: expect no line and `nor=0/0/0` every time.
  Run `dev suite` once (it now fails on a non-zero counter). If an error
  still appears, its kind is in the line: `cut` means a SPI0 user the
  mask does not hold off; `data`/`command` with `cut` 0 means a cause not
  in the code (U-NOR-4).
* Open: U-NOR-4 (what the controller really returns when cut; answered
  only if the old firmware is run with the counters, not needed).

### 2026-11-26 — settings store 192 keys and SYN-3 on the player; MIDI trips the watchdog (lead)

* Merged VER-4 store widening (`efad6eff`): index 128 → 192 keys, same
  NOR format; on the player the store came through the update identical
  (`store=ok keys=25 crc=acff8170`, `settings changed` unchanged); suite
  23/23 (stack free ≥ 7,824 B).
* Merged SYN-3 (`b2629690` → `1fda608d`): MIDI from Files/Music on the
  player, `synth bench`. Code pages 128/128 (full), stack margin 5,440 B.
* **`synth bench` on the player** (no DWT: µs × 144 MHz): all 36 loads
  bit-exact with the host goldens. Cycles per voice-sample: saw-lead 262,
  fm-epiano 452, strings 263, kick 296, snare 387, hats 695, cymbal 737,
  clap 158; per sample: delay 402, chorus 519, reverb 1,073; empty engine
  245 per frame — 1.7–3.7× the SYN-1/2 estimates (table:
  scratch `synth-bench-1fda608d.txt`). Budget at 44.1 kHz/144 MHz ≈ 3,265
  cycles per frame.
* **MIDI playback trips the watchdog**: `suite --setup midi` uploaded the
  three demo songs; `playdir /.orpheus/test/suite/midi` underran (`ub=-`)
  and the watchdog reset the player (last sample in
  `orpheus_synth::fx::Chorus::process`): chorus + reverb alone ≈ 1,600
  cycles per frame, a 10-voice song ≈ 4,800 — the RT render never yields
  and thread mode starves; the voice governor cannot shed the effects'
  fixed cost. The resume point was moved back to an MP3 (resume at
  start-up would reopen the MIDI file). A worker fixes the pacing and the
  governor. Do not play MIDI files on the player until then.
* The boot-time NOR read error recurred: `settings: store mounted after 1
  read errors` (the retry worked). Cause open.
### 2026-11-26 — SYN-3: load governor; MIDI playback can no longer starve thread mode (worker, no hardware)

The lead's run of build `1fda608d`: `playdir /.orpheus/test/suite/midi`
underran (`ub=-`) and the watchdog reset the player (last sample in
`Chorus::process`, IRQ 9); `synth bench` measured 1.7–3.7 × the SYN-1/2
estimates. Firmware build and host tests; **nothing run on the player**
(no `xtask dev`: the brief). Full "as built" in 04 §9 ("As built (SYN-3
load governor …)"); in short:

* **Cause**: the render ran on the real-time executor in 256-frame blocks
  and yielded only when the PCM ring was full; a song the CPU cannot keep
  up with never fills it, so thread mode (UI, USB, the watchdog's feeder)
  never ran. The old governor (−1 voice per late block, ≥ 4) could not
  recover the effects' fixed cost.
* **By construction now**: each piece is bounded (as many frames as the
  prediction fits in 4 ms, 32–256; bit-exact: the engine's output does not
  depend on the cut), and the pace rule rests 4 ms before a piece that
  would end 8 ms after the last rest or ring wait ≥ 4 ms
  (`orpheus_power::pace`, `audio::synth::pace_rest`): thread mode gets ≥ ⅓
  of any busy stretch whatever the song, the misestimate or the
  governor's state.
* **Governor** (`orpheus_synth::load`, host crate): per piece, the
  engine's estimate (`Engine::estimate`: voices by class, fading ones
  apart, effects that run) × the measured ratio (real/estimated, µs ×
  MHz) against **55 % of real time** (1,795 cycles a frame at 144 MHz);
  sheds the **chorus, the delay, the reverb, then voices** (limit for new
  notes; released voices fade first, oldest first; floor 2); effects fade
  out over 256 samples (`Engine::set_fx_enabled`); voices back after 1 s,
  effects after 3 s under 50 %. Order and reasons in 04 §9. Clock-aware:
  at 288 MHz (Performance: Maximum) the budget doubles. `synth gov off`
  for CRC checks.
* **Measured costs** in `orpheus_synth::load::PLAYER` and budgets.toml
  `[synth]` (replacing the estimates): empty 245, subtractive 155–308
  (table 280), FM 2-op 288 / 4-op 453, drums 159–737, **reverb 671,
  chorus 194** — the lead's 1,073/519 included the sine voice and its
  line loads (402); raw table `crates/orpheus-synth/tests/bench-player-1fda608d.txt`.
  The reverb is compute (+0.4 line loads a block), already half rate; a
  step-wise rewrite (bit-exact, −15 % on the host) has more instructions a
  frame on thumbv7em (313 vs 255) and was not kept: **no reverb speed-up**.
* **Paging**: `MidiPlayer::timed` and `Sequencer::handle` sat inside the
  render's physical lines (≥ 2 line loads, ≈ 180 µs each, per piece): new
  clusters `synth-midi` (lines 8–16: the governor's `plan_piece`/
  `after_piece`, the rest, `timed`, `handle`) and `synth-cold` (open/scan/
  seek, inits, patch install: fills lines 1–7 instead of padding).
* **Demo songs on the host model** (`orpheus-midi` `tests/governor.rs`):
  unshed 4,951 / 2,714 / 1,859 cycles a frame (Etude / Hill Song / Sound
  tour); at 144 MHz all three need the governor: reverb off most of the
  time, mean voice limit 2.2 / 4.2 / 6.1, < 1 % of PCM halves above 62 %
  of real time (one in Hill Song). At 288 MHz: Sound tour 91 % at full
  quality (bit-exact), Hill Song all 16 voices. Half rate (22.05 kHz,
  native DAC rate) would give the 288 MHz numbers at 144 MHz: considered,
  not built (rate-dependent engine state, a DAC rate change stops the
  ring; would be chosen at open) — **owner decision** (the top octave goes).
* **Logs**: `pcm: head … ub=synth:render:Nms` / `synth:rest` /
  `synth:read` (was `ub=-`); `status` `syn:` adds `lvl=now/max fx= load=
  ratio= rests= shed= gov=`.
* **Code room**: SHA-256/512 compact rounds in the firmware
  (`sha2_backend_soft="compact"`; the unrolled SHA-512 was 30.6 KB):
  **125/128 code pages** (was 128/128); `play_midi`'s poll kept within
  its 7-line hole (governor work in two `#[inline(never)]` functions, the
  rest an `Apart` future).
* **Numbers** (thumbv7em): resident **81,427 B** (+40 of 81,920), .bss
  **124,208 B** (+8: three `syn:` words), MIDI state **71,936 B** (+296:
  the effects' fade buffers 256, governor 40 vs 8), 13 cache blocks, 2,224
  B spare; engine 62,624 B; `MidiPlayer` 70,528 B; **stack margin 5,772 B**
  (14,452 of 20,224 B; was 5,440); `synth-render` 25 of 26 lines.
* **Tests**: `regress_synth_render_starved_thread_mode` (the loop model
  with today's figures: sustainable after 10 pieces, thread mode ≥ 44 % of
  every 100 ms, pieces ≤ 5.2 ms, busy ≤ 7.1 ms, 0 underruns), an
  unfixable overload still ≥ 30 % thread mode, governor order/restore/
  bounds/ratio/edge tests; `regress_synth_underruns_are_labelled`; engine:
  all-on is bit-exact, fade-out then exactly dry, `shed` order, estimate
  classes; `orpheus-midi` `tests/governor.rs` (the three songs at 144 and
  288 MHz, half rate, a 1.5 × misestimate). All synth/MIDI goldens and
  play CRCs unchanged.
* **Gate**: fmt (host, fw); clippy `-D warnings` (host workspace all
  targets, fw workspace); `cargo test --workspace` 2,035 passed, 0
  failed, 13 ignored; `settings-matrix --check` (174 settings, 0 errors);
  fw `xtask build` (ok, 125 code + 26 rodata pages, no cluster warnings),
  `check-resident` (ok, none paged), `size --check` (resident 81,427 B,
  .bss 124,208 B), `stack --check` (14,452 of 20,224 B, margin 5,772 B;
  `synth-render` 25 of 26 lines). Not run: any `xtask dev` step (the
  brief), `brand`/`site --check`, fuzzing beyond the corpus replay.
* **For the lead on the player** (`playdir` etc. as below):
  1. `synth bench 32` and `synth bench 29` — the reverb and its dry
     baseline, to confirm 671 (the hashes must equal
     `goldens-bench-player.txt`: the engine is unchanged with all effects
     on).
  2. `playdir /.orpheus/test/suite/midi`, then `status` every ~20 s for
     3 minutes: **no reset**; `syn: lvl=` mostly 2–3 at 144 MHz, `rests=`
     growing only while the ring is short, `ratio=` near 100 (the table
     fits; ≫ 100 means paging or patches the table underrates),
     `underruns` 0 or a burst at a song's start only, `ub:` labels
     `synth:…` if any; the UI responds throughout (Now Playing, menus,
     `xtask dev shot`).
  3. CRC: `synth gov off`, `dsp off`, `crc on`, then `play
     "/.orpheus/test/suite/midi/Etude.mid"` (and the other two): `frames=`
     and `crc=` must equal `crates/orpheus-midi/tests/goldens-play.txt`
     (Etude 1131268 / 54821393, Hill Song 1146600 / 1777526454, Sound
     tour 2469600 / 4099842480) — with the governor off these stay
     bit-exact; it plays slower than real time (underruns expected, no
     reset, UI alive). With `synth gov on` (the default) **no song stays
     bit-exact at 144 MHz** (the governor acts on all three); at
     Performance: Maximum "Sound tour" is at full quality for 91 % of its
     length, also not bit-exact as a whole.

### 2026-11-26 — VER-4: settings store index 128 → 192 keys, same NOR format (worker, no hardware)

* **Problem**: with every app's settings the store's index was full
  (`MAX_KEYS` 128; STORED 128). With the Bluetooth address (`0x06F0`),
  the radio presets (`0x0710`) and the retired Brightness key (`0x0301`)
  **131** keys were needed (the firmware alone 129): after a factory reset
  the last writes were refused (`TooManyKeys`, ignored by the firmware;
  those values reverted at the next start). The settings cache had its
  own limit too (a `u128` write mask).
* **Limits now**: store index **192 keys** (`MAX_KEYS`; `Store`'s new
  third parameter `K`, default 192); cache write marks a bitset sized from
  `STORED` (no limit of its own). In use: full tree 131 (61 free),
  firmware 129 (63 free). `settings::STORE_KEYS` / `MIN_FREE_STORE_KEYS`
  (32) asserted at build time in the firmware (`ui.rs`) and in the host
  tests. `bt::LAST_KEY` is now `orpheus_apps::settings::BT_LAST_KEY`.
* **Sector capacity is the real ceiling** (not changed: 4 sectors at the
  same place): only the active 4 KiB sector holds live records, 4,080 B of
  them. A two-byte setting is a 16-byte record, the presets 344 B, the
  Bluetooth address 20 B. 192 keys of that mix (190 settings + presets +
  BT) = 3,404 B, leaving 676 B (42 small writes) between compactions; the
  presets can still be rewritten up to ≈ 209 settings keys; **256 keys
  cannot fit one sector** (256 × 16 B > 4,080 B) without a format change
  (live records across sectors) — so 192, where the index is full before
  the sector.
* **Format unchanged, both ways**: the index is RAM only. The new code
  writes byte-for-byte the same NOR image as the old for the same
  operations (`new_code_writes_the_old_format_byte_for_byte`, against a
  fixture the old code wrote, `crates/orpheus-settings/tests/data/
  store-128keys-c3041aeb.bin`); the old image mounts with identical values
  under a 192- and a 128-key index, and the mount writes nothing. An older
  firmware reading a store with more than 128 keys sees the first 128 in
  log order and drops the rest at its first compaction, nothing else
  disturbed (`smaller_index_reads_a_bigger_store`); today's firmware
  stores ≤ 129, so at worst one key. The owner's store (25 keys) is read
  as it is: the boot's `keys=` count and store CRC (`STORE_KEYS`,
  `STORE_CRC`, index order unchanged) should be identical before and
  after the update.
* **RAM**: index slot 8 → 4 B (key + 15-bit offset in the active sector
  + deletion bit; the length is read back from the record header), so 192
  keys = 768 B (was 1,024 B for 128): `STORE_CELL` 1,056 → 800 B. The
  compaction copy and the CRC check stream in 64-B pieces instead of two
  1 KiB buffers, and the boot load no longer copies the key list (268 B):
  the UI task's future (`ui::ui::POOL`, .bss) 1,520 → 536 B. **.bss
  125,448 → 124,200 B (−1,248)**; usable stack 19,200 → 20,480 B; **static
  stack margin 4,304 → 5,664 B**; resident 81,235 B unchanged.
* **Compaction** now rebuilds the index by scanning the new sector once it
  is active (the index is untouched until then: a NOR error part-way
  leaves RAM on the old sector, still the active one), and copies each
  record with its own CRC after checking it. **Fixed**: a value that
  rotted in NOR after the mount was copied with a fresh CRC and became
  valid (`regress_compaction_does_not_bless_a_rotted_value`, fails on the
  old code).
* **Tests**: `orpheus-settings/tests/compat.rs` (fixture mounts, byte
  equality, smaller index, `full_index_before_full_sector`: 192 keys and
  2,000 writes incl. 332-B values never `Full`, the 193rd key refused;
  `power_cut_at_every_step_with_many_keys`: 180 keys + 6 new + removes,
  a cut at every one of the workload's write steps through compactions,
  with the fuzz NOR model's no-program-over-programmed-bits check);
  `orpheus-apps` `regress_full_feature_settings_tree_fits_with_headroom`
  (the real store after a factory reset on a player with the retired key,
  BT and full presets: ≥ 32 keys free, 32 new settings stored, six rounds
  of rewrites through compactions, all values back after a remount),
  `dirty_marks_cover_every_stored_item`; new fuzz seed
  `seed-many-keys` (180 keys); the corpus replays.
* Gate: fmt (host, fw); clippy `-D warnings` (host workspace, fw;
  `orpheus-apps` with no features, `seq`, `games,salvo,bricks`, `books`);
  `cargo test --workspace` 1,995 passed, 0 failed, 13 ignored;
  `settings-matrix --check` (174 settings, 12,224/12,272 cells, 0 errors);
  fw `xtask build`, `check-resident` (ok, none paged), `size --check`
  (resident 81,235 B, .bss 124,200 B), `stack --check` (14,816 B of
  20,480 B, margin 5,664 B). Not run: any `xtask dev` step (the brief
  forbids them), `brand`/`site --check`, fuzzing beyond the corpus replay.
* **For the lead on the player**: before the update `settings changed`
  and the boot line's `store=ok keys=N` / store CRC; update; after it the
  same `keys=N`, the same CRC and the same `settings changed` (the
  owner's `701=1 d50=10470` among them).
* Open: the index still counts deletion markers until the next
  compaction (only `remove` makes them; the firmware does not call it);
  F-SET-2 unchanged.
### 2026-11-26 — SYN-3: the synthesiser on the player; MIDI files play (worker, no hardware)

Firmware build and host tests; **nothing run on the player** (no `xtask
dev`: the brief). Full "as built" in 04 §9 ("As built (SYN-3 …)"); in
short:

* **Source switch** (`orpheus_audio::synth`, host-tested): `.mid`,
  `.midi`, `.rmi`, `.kar` play through the synthesiser, everything else
  through a codec; a synth item is never gapless (a MIDI file the gapless
  reader read ahead is opened fresh after the previous track played out).
  `play PATH` and the queue dispatch on it; `orpheus-apps`' `midi` feature
  is on in the firmware (Files and the indexes list MIDI files as audio).
* **Placement**: the stream ring and the decoder arena are one `repr(C)`
  static `POOL` (same bytes; .bss **125,448 B, unchanged**). A synth
  source borrows both as one 80,816-byte region (`StreamRing::lend`; the
  library state stays) and lays it out with `orpheus_audio::synth::carve`:
  **MIDI state 71,640 B** on thumbv7em (`MidiPlayer` 70,264 = engine 62,360
  + parser 6,224 + …; cache tags, governor, a 256-frame block), 13 card
  blocks, 2,520 B spare; a const assertion keeps it fitting. Built in
  place: every engine/MIDI type is `bytemuck::Zeroable` (enums `repr(u8)`,
  `Option` fields → `Opt`/a sentinel), the firmware zeroes and casts (one
  `unsafe`), `Engine::init`/`MidiPlayer::init` fill it field by field
  (`new` = zeroed + `init`: one path; tests: `init` on a used engine or
  player equals `new`).
* **Card source** (`orpheus_midi::cache::BlockCache`, host-tested): the
  parser's ≤ 64-byte `Need`s from 512-byte blocks, LRU, one sector read
  per miss with `Urgent`; the golden songs render bit for bit through 2, 4
  and 32 blocks, seeks equal the slice source's.
* **Playing** (`audio::synth::play_midi`): open + scan with the DAC
  stopped (breaks for thread mode), 256-frame blocks at **44.1 kHz**
  through the DSP chain into the PCM ring, requests, pause/fade, seek
  (re-scan, DAC stopped), stop/skip, card requests and listing pieces as
  chores (`chore::decide`), battery/step logs; Now Playing shows codec
  MIDI and the scan's duration. **Clock floor High** while it plays.
  **Load governor** (`VoiceGov`, host-tested): a block's render above 60 %
  of its real time drops the voice limit by one (≥ 4), calm (< 35 % for
  180 blocks) brings one back; `synth voices N` caps it. `status` ends in
  `syn: v= vlim= over= reads=`.
* **Bench** `synth bench I [N] [BLOCKS]` (read-only, no card access): 36
  loads (`orpheus_synth::bench`), cycles from the DWT counter (new
  `atj2157_hal::cycles`; U-DWT-1: unknown whether the chip has it, else
  µs × MHz, `cyc_ok=`), pager loads, voices, and the render's hash —
  `crates/orpheus-synth/tests/goldens-bench-player.txt` holds the player's
  (the delay loads 29, 30, 35 render dry there), `goldens-bench.txt` the
  host's. Without arguments it prints the sizes.
* **Whole MIDI songs bit-exact through playback**: `dsp off` + `crc on` +
  `play PATH` replies `frames=`/`crc=`; `crates/orpheus-midi/tests/
  goldens-play.txt` predicts them for the suite's three MIDI fixtures
  (`xtask dev suite --setup` now uploads `orpheus_midi::demo::SONGS` to
  `/.orpheus/test/suite/midi/`).
* **Code room** (the code window was 121/128 pages; the synthesiser is
  ≈ 65 KB): `orpheus-synth` features `samples` and `delay` (default on)
  are **off in the firmware** (no card sample source yet; no MIDI
  controller feeds the delay — with it off the delay keeps its settings
  and adds nothing, MIDI renders are unchanged); the sequencer's channel
  callback is `&mut dyn FnMut`; one filter/envelope tail in the voice
  render; GM/kit/bench tables linked without names; `synth midi`/`synth
  golden` commands dropped for the playback CRC check. **128/128 code
  pages**, rodata 26/48. Clusters: `synth-block` (the block loop, 7 lines)
  in the window's hole before `flac-frame` (no page), `synth-render` (16
  lines: render, SVF and envelope steps, kick, snare, hats, clap; budget
  26 checked by `stack --check`), on physical lines 1–7 and 17–31 + 0.
* **Resident/stack**: panic location data is resident, so the new paths
  in `orpheus-midi`, the engine, the reverb and the kits were made
  panic-free (`get`, index masks) — resident **81,388 B** (+153 B:
  constant pools, strings), **stack margin 4,336 B** (was 4,304; usable
  19,200 B). `stack.toml`: the sequencer's `dyn FnMut` targets,
  `serve_one`'s waker call.
* **Measured** (host / build): sizes above; host ns per voice-sample
  unchanged from SYN-1/2. **Not measured: cycles per voice and effect on
  the player** — budgets.toml `[synth]` holds SYN-1/2's estimates
  (saw-lead ≈ 140, FM 4-op ≈ 260, snare ≈ 230, reverb ≈ 290 per sample …)
  to be replaced by the bench's numbers.
* **Tests**: `orpheus-audio` (lend, switch rules, carve, governor),
  `orpheus-midi` `cache` (6) and `tests/card.rs` (goldens through the
  cache, seek, `init` = `new`, play CRCs), `orpheus-synth` `tests/bench.rs`
  (36 bench hashes, `init` = `new`, scenarios set up in place, streaming
  hash), suite journey **`midi`** (bench hash, the fixtures play without
  underruns with voices sounding; `SimPlayer` runs the host bench); all
  synth/MIDI goldens unchanged (the refactors are bit-exact). No bug found
  in the existing code (no `regress_` test).
* **Gate**: `cargo fmt --all --check` (host, fw); `cargo clippy --workspace
  --all-targets -D warnings` (host) and `cargo clippy --workspace -D
  warnings` (fw); `cargo test --workspace` **2,011 passed, 0 failed, 13
  ignored**; `cargo xtask settings-matrix --check` (174 settings,
  12,224/12,272 cells, 0 errors); `cargo xtask build` (128/128 code, 26/48
  rodata pages), `check-resident` (ok, none paged), `size --check`
  (resident 81,388 B, .bss 125,448 B), `stack --check` (margin **4,336 B**,
  `synth-render` 25 of 26 lines). **Not run**: `brand`/`site --check`,
  `xtask test all` beyond these, every `xtask dev` step (the brief: no
  hardware), `cargo test -p orpheus-synth --no-default-features` beyond
  `--test bench` (the other goldens include the delay).
* **For the lead on the player** (after staging this build):
  1. `cargo xtask dev cmd 'synth bench'` → `… loads 36 midi_state=71640
     pool=80816 engine=62360 lib_state=1104`.
  2. Bench, each read-only (≈ 0.1–1 s): `cargo xtask dev cmd 'synth bench
     I' --wait 10` for I = 0…35; compare `hash=` with
     `crates/orpheus-synth/tests/goldens-bench-player.txt` line I (bit-exact
     f32 on the player, FZ = 0); note `cyc_ok` (U-DWT-1),
     `cyc_per_frame`, `loads`, `voices_x100`. Per voice = (load −
     load 0) × 100 / `voices_x100`; per effect: `synth bench 29|31|32 1`
     − `synth bench 1 1`. Also `synth bench 9 16` (16 FM e-piano voices,
     the worst melodic case). Into budgets.toml `[synth]`.
  3. Fixtures: `cargo xtask dev suite --setup midi` (uploads the tones and
     the MIDI songs under `/.orpheus/test/suite/`, runs journey `midi`).
  4. Bit-exact songs and underruns: `cargo xtask dev cmd 'dsp off'`,
     `cargo xtask dev cmd 'crc on'`, then `cargo xtask dev cmd 'play
     /.orpheus/test/suite/midi/Etude.mid' --wait 40` → `played
     frames=1131268 … underruns=0 … crc=54821393`; `Hill Song.kar`
     (`frames=1146600 crc=1777526454`), `Sound tour.mid` (`--wait 70`,
     `frames=2469600 crc=4099842480`); after each `cmd status` (`syn:
     over=0`: else the governor lowered the limit and the CRC differs).
     Then `cmd 'dsp on'`, `cmd 'crc off'`.
  5. With the DSP on and browsing: `cmd 'playdir
     /.orpheus/test/suite/midi'`, open Files/Music, list a large folder
     while it plays, 60 s, `cmd status` → `underruns=0`, `ub:` all 0,
     `syn: vlim=16 over=0`, `dec: max_us=` (the longest render block).
  6. Paging: `cargo xtask dev prof --faults 30 --during 'cmd play
     /.orpheus/test/suite/midi/Sound tour.mid'` — the `synth-render`
     cluster should fault little after warm-up.
* **User check 33** (docs/USER-CHECKS.md): listen to the three MIDI
  fixtures on the player (the lead starts them with `playdir`; or the
  owner's own `.mid` files): as the host WAVs (`cargo run --release -p
  orpheus-midi --example render -- DIR`), no clicks or dropouts; seek and
  pause in Now Playing.
* **Open / next**: the measured cycles; whether a light song may run
  below High (a floor by voice count once the numbers exist); lyrics and
  the mute/solo/tempo view in Now Playing; the card `SampleSource`
  (samples are compiled out); the code window is full — the Sequencer on
  the player needs room for its own code and the delay (≈ 2.4 KB), e.g. by
  moving rarely used code out of the window or growing it.
* **What the Sequencer needs on the player**: the engine is in place in the
  pool (`Engine::init` on zeroed memory: 62,360 B on thumbv7em) and leaves
  ≈ 18 KB of the 80,816-byte pool; the project (35.8 KB) does not fit
  beside it — split it as SEQ-2 noted (synth steps and locks elsewhere) or
  shrink it; `render_i16` from the playback loop as `play_midi` does
  (blocks into `Out`/`Feeder`, the governor, chores); the delay feature
  back on (code room). **Speech**: the TTS synthesiser (`orpheus-tts`,
  ≈ 3.4 KB) is small enough for the pool or its own static; it can use the
  same synth-source loop (a `Speaker` rendering blocks) — not started.

### 2026-11-26 — Brightness merged; hex keys; settings store mount retried (lead)

* Merged Brightness (`777579cf`: stock levels 1–11, default 6 = duty 8,
  new key 0x030D, 0x0301 retired; alert flash capped at 15; stop/start
  duty writes VERBATIM). Suite on it: the new `settings_brightness`
  journey failed — the firmware's test-command parser took only lower-case
  hex and refused `settings test-set 0x030D` (`e815359e`:
  `orpheus_proto::num::parse`, `regress_upper_case_hex_keys_parse`). On
  the player: `bl=8 → 3 → 15 → 8`, passes.
* **Settings store not mounted once**: the full suite on `e815359e` passed
  23/23 but `settings changed` came back empty — the boot after the
  brightness run reported `store=none keys=0`: `Store::mount` fails only
  on a NOR read error; nothing was written (with no store the firmware
  writes no settings). After a plain reboot: `store=ok keys=25` and the
  owner's settings back (`701=1 d50=10470`). Fix: `Store::mount_retrying`
  (3 tries, fresh driver; never writes on a read error;
  `regress_mount_survives_a_read_error`), a log line when it needed
  retries or failed, and the suite fails on `store=none`
  (`regress_suite_sees_an_unmounted_settings_store`). Open: the read
  error's cause (once in many boots).
* Settings coverage: 3 settings without an effect (Speed hidden, Wallpaper,
  Monitor).

### 2026-11-26 — VER-13 follow-up: Brightness on the stock levels, alert flash capped at 15 (worker, no hardware)

* **Brightness works** (U-PWM-3): `orpheus_power::backlight` holds the
  stock table (level 0–11 → duty 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15 of
  `DUTYMAX` 16). `Duty` can only hold a value of it (`Duty::of_level`
  clamps any `i16`; host test over every `i16`:
  `every_level_maps_into_the_stock_table`), and the BSP's
  `Backlight::set(Option<Duty>)` takes nothing else, so no duty outside
  the table can reach PWM4. Settings › Display › Brightness is 1–11,
  default 6 (duty 8, the stock default; 05 §7.11 gives no range), applied
  at once while the slider or the quick panel's row moves.
* **Old stored value**: the setting was `0x0301`, 1–16 default 10, and
  drove nothing (fixed duty 12). Its value cannot be told apart from a
  new level under the same key, and after a factory reset every player
  stores 10 there — which would have read as level 10 (duty 13) and shown
  in `settings changed`. So the key is **retired** (`settings::RETIRED`,
  never reused) and Brightness is **`0x030D`**: an old store reads the
  stock default 6 and nothing is written (test
  `old_brightness_record_reads_the_stock_default`, old values 1, 6, 10,
  11, 12, 16, −1). The stale `0x0301` record stays in NOR, skipped at
  load (the store's key count and CRC at boot are unchanged by it).
  **Every reference to "Brightness 0x0301" is now 0x030D** (manifest,
  matrix test `s030d`, fuzz and soak id lists).
* **Duty writes use the vendor's stop/start** (U-PWM-1 still unmeasured):
  `atj2157_hal::pwm::Pwm::set_duty_restart` (CTL bit 4 cleared, DUTY,
  bit 4 set, no wait; VERBATIM), then `enable` and the pad mux as before.
  Off stays the pad driven low. The 20 ms wait before muxing the pad is
  still not copied (it never was).
* **Alert flash capped at 15** (bright) / 3 (dim): the flash is part of
  `orpheus_power::backlight::light`, one function for the firmware and
  the simulator (`services::backlight`); `regress_alert_flash_never_drives_16`.
  The firmware's backlight is now set in one place (`ui.rs`
  `apply_backlight`, every loop, written only on change; boot at the
  stock default, the stored level right after the settings load).
* **Dim**: not built. 05 asks for no dim state before off (§6.3's saver
  "dim clock" is drawn dim, as built); Battery saver's "backlight capped"
  (02 §5.4) is still open — the Power mode entry's `backlight` reason now
  says so.
* **Effects**: `Sim::backlight()` (the duty, 0 dark); the matrix compares
  it beside every screen context's frame, so Brightness shows in every
  lit screen context except Alert (the flash ignores the level) and leaks
  nowhere; no other setting moved it. Manifest `0x030D`: no `gap`;
  `backlight` covered by `effects::brightness_lights_the_panel_at_its_level`,
  `services::setting_effects::brightness_values`, the power test and
  `suite:settings_brightness`. `settings-matrix --check`: 166 settings,
  11,542/11,590 cells (99.6 %), 3 settings without an effect (was 4),
  0 errors.
* **Player observable**: `ui top` ends in `bl=<duty>` (0 dark; the same
  atomic the battery log's `backlight` column reads, now a duty);
  `xtask` `Top::bl`. Journey **`settings_brightness`** (RAM-only session):
  `bl=8` under the test look, `test-set 0x030D 1` → `bl=3`, `11` → `bl=15`
  and the slider shows 11 (shot `slider_11`), back to 6 → `bl=8`. The test
  look reads Brightness as its default until a session sets it
  (`LOOK_ARMED`, which replaces `A11Y_ARMED`: bits 1–3 the accessibility
  settings, bit 4 Brightness; no new RAM).
* Goldens: 19 re-blessed (Display group and slider, 10 → 6 / 13 → 9;
  looked at), `docs/shots/app_display_dark_latin.png`, suite
  `settings/display.png`, new `settings_brightness/{slider_11,home_after}`.
  User docs (`docs/user/settings.md`), 05 §7.11 as-built, 01-hardware §9
  U-PWM-1/U-PWM-3 updated. **User check 32**: each level by eye against the
  reference player.
* Gate: fmt (host, fw), clippy `-D warnings` (host, fw), `cargo test
  --workspace` 1,932 passed, `settings-matrix --check`, `site --check`;
  fw `xtask build`, `check-resident` (ok, none paged), `size --check`
  (resident 81,235 B of 81,920; `.bss` 125,448 B of 126,976, unchanged),
  `stack --check` (margin 4,304 B, unchanged). Not run: `xtask dev`
  anything (no hardware).
* **For the lead on the player**: `suite settings_brightness`; then look:
  Settings › Display › Brightness steps change the light at once (the
  stop/start sequence), the alert flash, and that the owner's player now
  starts at level 6 (darker than the old fixed 12). Open: U-PWM-1 (is the
  stop/start needed), the vendor's 20 ms before the pad mux.

### 2026-11-26 — SEQ-2: synth tracks, locks, song, export on the host and in the simulator (worker, no hardware)

Host and simulator only; the `seq` feature stays off in the firmware
(SYN-3). Full "as built" in 05 §7.16 ("As built (SEQ-2 …)"); in short:

* **Model** (`orpheus-seq`): tracks 8–9 are synth tracks (bass, lead):
  4-byte `SynthStep` (the drum step's fields + note, gate ¼–64 steps,
  chord single/triad/seventh/power built in the scale; a chord plays
  once), per-track patch (preset or edited), level, sends, mute/solo
  (solo across all ten tracks); pattern transpose ±24; scale lock (10
  scales × 12 roots); per-pattern lock store of 24 locks (drums: tune,
  decay, tone; synth: cutoff, resonance, glide) that refuses the 25th and
  changes nothing; kit B (each drum track picks A or B); the fill pattern;
  song editing API (insert, set, remove, move).
* **Player**: synth note-offs computed with the note, kept per track (8
  sounding notes), always on the first sample of a render piece before its
  note-ons; Stop ends every synth note; drum locks through three new
  engine parameters `Param::DrumTune/DrumDecay/DrumTone` (offsets for the
  part's next hits, `orpheus-synth`), synth locks through the part's
  cutoff/resonance/glide (only changes sent); metronome (own part), count-
  in 1–2 bars, step length 1/16·1/8·1/32 (exact), fill while held, notes
  from the keys (`Sequencer::live`, `Studio::key_note` with live record
  quantised to the nearest step), `quantize`. More than 64 events in a
  piece are rendered in turns (an empty render applies events).
* **File** `OSQ1` version 2 (records 6–9: synth steps with transpose, kit
  B, synth tracks with their patch as `OSB1`, locks); SEQ-1's version-1
  files load unchanged (tested against the files SEQ-1's code saved, kept
  in `crates/orpheus-seq/tests/data/`), in the app too (a v1 file on the
  card opens and saves as v2). New project 924 B, demo song 3,567 B,
  `MAX_FILE` 36,041 B.
* **Export**: `export::WavExport` (offline, its own player, any piece size,
  song + 2 s) and `export::midi` (SMF type 1, 600 ticks a step so every
  position is an exact tick; drums ch 10, bass/lead ch 1/2 with GM
  programs and CC 7) → `/.orpheus/seq/SEQnn.WAV` / `.MID` (rule 5).
* **Screens**: grid with 10 tracks (8 rows, the synth tracks scroll in;
  chords dotted, locked steps marked, the note in the footer), its menu
  (+ Sound, Keys, More › Live, Song, Export WAV, Export MIDI: a menu holds
  12), step (+ note, length, chord, three lock rows), Sound (drum kit
  entry / synth patch), Keys (one-octave strip, scale lock, step entry,
  hold OK = undo, Record), Live (hold OK = fill, ◀▶ next pattern, ▲▼
  tempo, count-in shown), Song (entries, ◀▶ pattern, OK +1 repeat, Add,
  Insert/Delete/Play the song), Sequencer settings (list › Menu ›
  Settings). New strings (55) are English and behind the `seq` feature,
  appended last in `Str` (strings added later go above them).
* **Settings** (VER-13): Default kit, Default length, Default swing,
  Metronome, Count-in, Step length, Key clicks in play (word `0x1180`,
  list `0x11`, only with `seq`: the firmware's tables are unchanged).
  New service context `seq-play` (a new project's defaults; the demo beat
  played 2.5 s dry; key clicks while playing); Rotate 0x0309 declared
  there too (its swapped keys open another project). `settings-matrix
  --check`: 174 settings, 12,148 / 12,196 cells, 0 errors. **The settings
  store is now full with games + Books + Sequencer: 128 of 128 keys**
  (firmware: 126); the cache's "reset" mask handles 128.
* **RAM** (host `size_of`): project 35,808 B (SEQ-1 25,092; pattern
  2,147 B), player 3,784 B (1,880), studio 40,144 B, WAV export 3,808 B,
  kit B 289 B; engine 62,320 → 62,512 B (drum offsets of 16 parts).
* **Tests**: `orpheus-seq` lib 6, `file` 11 (v1 migration against SEQ-1's
  files, synth/locks round trips, impossible v2 records, a full lock store
  refuses), `synth` 11 (synth note on/off frames exact against seconds at
  8/44.1/48 kHz × 40–240 BPM × swing, no drift over 650 loops, chords /
  transpose / ratchets, sounding-note table, locks at their step dry and
  as sound — drum locks bit-identical to the entry changed by hand, synth
  locks equal to the engine fed by hand —, live record quantisation with
  swing and across the pattern end, fill, metronome, count-in, step
  length, kit B), `export` 6 (WAV header/length, any piece size = Play,
  **hash of the demo song WAV at 22,050 Hz `0xf3cd9d8f0d176097`**, MIDI
  ticks exact at each step length, MIDI parsed back by `orpheus-midi`
  equal to the dry run, bounds), `play` 19 (+ densest every-track queue),
  `timing` 6, fuzz corpus (17 seeds incl. SEQ-1's v1 files; oracle now
  checks every synth note ends once after a stop, metronome/count-in/
  fill/step length options, MIDI export): `cargo +nightly-2026-11-05 fuzz
  run seq_load` 543,072 runs in 151 s, no finding; `orpheus-synth`
  `drum_offsets_change_the_next_hits_only`; `orpheus-apps` settings table.
  Simulator: 16 new journeys in `sim/tests/seq.rs` (note entry, scale
  lock, live record, locks from the step screen incl. full store, sound
  screens, song editing, fill on the live screen, WAV/MIDI export and a
  failing card, a SEQ-1 file opens, 7 `settings_effects`); 40 new goldens
  `seq_*` (SEQ-2 journeys, Dark/Light, 2 Amber) and 17 SEQ-1 goldens
  re-blessed (scroll bars of longer lists, the list menu's Settings, the
  track screen's Kit row, the grid menu's last item) — all looked at.
* **WAV for the owner**: `tram-lights.wav` ("Tram Lights", 104 BPM, swing
  54 %, A minor, punch + percussion kits, acid bass, FM piano chords with
  delay/reverb, bass cutoff locks, snare tune locks in the fill pattern;
  A A B B C C A B D, 27.4 s, peak −0.7 dBFS) with `.mid` and `.osq`, and
  `night-bus.wav` (SEQ-1's beat through the WAV export: 24.0 s, peak −2.8
  dBFS as before) in the session scratchpad `seq2/` (not committed):
  `cargo run --release -p orpheus-seq --example render -- <dir>`.
* **Gate**: `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -D warnings`; `cargo clippy -p orpheus-apps --all-targets
  -D warnings` with no features, `seq`, `seq,games`, `seq,books`,
  `games,salvo,bricks`, all; `cargo test --workspace` 1,974 passed, 0
  failed, 13 ignored (was 1,910); `cargo xtask settings-matrix --check`
  (above); `cargo xtask build`, `size --check` (resident 81,235 B, bss
  125,448 B), `check-resident`, `stack --check`: pass, **margin 4,304 B
  unchanged** (no new firmware strings or tables). **Not run**: `xtask
  test all` beyond these, `brand`/`site --check`, every `xtask dev` step
  (the brief forbids them).
* **User check**: listen to `tram-lights.wav` (bass line and its filter
  opening in B, the piano chords, the breakdown, the fill's rising snare);
  open `tram-lights.mid` in a DAW (tempo, 4 tracks, channel 10 drums). In
  the simulator: `set:seq=on`, Menu › Keys on a synth track, More › Live.
* **Open**: USB MIDI clock out (optional, M16); early micro-timing (a
  lookahead) still not built; the lead may want the settings store's last
  key back (pack the Sequencer word elsewhere or widen the store).
* **For SYN-3 (the Sequencer on the player)**: the engine (62.5 KB) in the
  decoder arena; the project is now **35.8 KB — 3 KB more than the idle
  32 KiB stream ring**: split it (e.g. synth steps and locks, 9.8 KB, into
  the arena's remaining ≈ 9.5 KiB) or shrink it (patterns or lock
  capacity); the player 3.8 KB; `render_i16` from the synth task while
  `Sequencer::busy()` (live notes sound while stopped), its 64-event
  buffer (1 KB) on that task's stack; `key_note` with the DAC frame
  (clock − ring latency) for quantisation; `seq::key_click_allowed` in the
  firmware's `key_click`; a BASEPRI lock (or messages) around project
  edits vs a render block; WAV export in the card task through
  `WavExport` pieces with the engine borrowed while stopped (faster than
  real time to be measured); MIDI export needs a streaming writer (track
  lengths patched after) or the stream ring as its buffer (the demo song's
  file is 4.3 KB); turning `seq` on brings SEQ-1's and SEQ-2's strings
  (≈ 2 KB read-only data: the stack margin needs a 256-B step of room)
  and the settings word (127 keys without Books).

### 2026-11-26 — Speed hidden, U-PWM-3, A11Y-2 and SEQ-1 merged; USB cable fault (lead)

* **Speed off the menu** (owner 2026-11-26: "take speed off the menu for
  now"; `7cd85f89`): `settings::hidden` — Speed (0x0206) is kept (key and
  stored value) but offered neither in Now Playing's menu nor in Settings ›
  Playback; the library goldens that counted Now Playing's menu rows now
  count one fewer.
* **U-PWM-3 answered** (`077829db`, from the stock `DRV_LCD.DRV` and the
  apps' common UI code): duty table 2,3,4,5,6,7,8,9,10,11,13,15 of
  DUTYMAX 16 for levels 0–11, default level 6 (duty 8), dim = duty 2; the
  vendor stops the channel around a duty write (VERBATIM; U-PWM-1 still
  unmeasured). The stock firmware never writes 16: cap Orpheus's alert
  flash at 15. Brightness (0x0301) can now be built.
* **USB link fault (hardware)**: from 21:57 the player kept dropping off
  USB — descriptor reads failing with -71, the host resetting the link,
  the firmware's USB-W1 recovery (`recover-reset`, setups 0) running as
  designed; workstation port 11 then port 7 both failed. The owner swapped
  the cable: since then no unexpected disconnects. Not a firmware fault.
* Merged A11Y-2 (`b7479505`: Accessibility group, Text size, Hold delay,
  Key repeat, high-contrast themes, reduced motion, hold click) and SEQ-1
  (`7565f73b`: `orpheus-seq`, the Sequencer app in the simulator). Manifest
  conflicts merged as unions of the context lists; Text size declared on
  the Sequencer's project list (`s1001` caught it). 1,924 host tests;
  `settings-matrix --check` 166 settings, 99.6 %, 0 errors; fw: resident
  81,235 B, `.bss` 125,448 B, stack margin 4,304 B.
* Suite on `0aa98981`: **22/22**; idle Home 871 line loads/s (limit
  1,500); stack free ≥ 6,544 B; owner's stored settings unchanged (the
  owner's own radio change, Region 1 and 104.7 MHz, is the new baseline).
  One intermittent `settings` journey failure earlier (first key not taken
  right after a reboot; passed twice alone): watch.

### 2026-11-26 — A11Y-2: large text, high contrast, reduced motion, hold and repeat, key confirmation (worker, no hardware)

* **Where**: a thirteenth Settings group, **Accessibility** (keys
  `0x1001–0x1003`, high byte `0x10`, packed into the word `0x0D60`: one
  cache slot, 126 of 128 with every feature). High contrast is two more
  Themes (Display), reduced motion is Animations off (Display), the key
  confirmation sound is Key click (Sound) — no duplicate rows. As built in
  05 §7.14.
* **Text size** (`0x1001`): `orpheus_ui::font::TextSize`, `FontId::{Large,
  LargeBold, XLarge, XLargeBold}`, `Gfx::text`/`Gfx::ui`,
  `System::text_size`. Large = the Books 16 px face (`BOOK16`, now always
  built, bold emboldened); Extra large = the UI font doubled (the spec's
  "twice the size", no rodata). BOOK20 stays behind `book-fonts`: at 20 KB
  it is over the 16 KiB rodata group and would be resident (765 B left).
  Lists 19/24/32 px rows (5/4/3 rows), headers bold, label keeps ≥ 2/3
  beside a value, at Extra large a row that does not fit draws in 16 px;
  keys page by the rows drawn (`ListView` remembers them: no screen
  changed) and the focus stays visible; menu sheets ≤ 110 px; strip and
  dialogs in 16 px at both sizes (dialog: two-line title, up to four text
  lines, over the status bar when needed); Now Playing Standard without
  art, title over the width (largest face without broken words), artist,
  album, queue place, times in 16 px; idle view without its icon.
  Unchanged: status bar, big numerals, games, pictures, reader, Steps,
  Authenticator, Equaliser, lock/charging, self-drawn lists.
* **High contrast**: `Theme::ContrastDark` ("White on black") and
  `ContrastLight` ("Black on white"), accent ignored. Test over every
  pairing the toolkit draws text in, as drawn after RGB565 and blends
  (`theme::tests::high_contrast_themes_reach_7_to_1`): lowest 7.30:1 and
  7.76:1. **Finding** (`contrast_table`): in Dark/Light a focused row's
  value (3.6/4.3:1) and an unfocused destructive dialog button (3.7:1) are
  under 4.5:1; not changed here (would move every golden).
* **Reduced motion** (Animations off): spinner a still ring, the paused
  recorder's dot and the alarm alert's icon stay lit, the radio's spinner
  is drawn once. No screen slide exists; no blinking cursor exists; the
  saver clock still jumps once a minute (burn-in, 05 §6.3). Games keep
  their own motion.
* **Hold delay / Key repeat** (`0x1002`, `0x1003`): `Timing::accessible`
  (Power's menu at 1.5 s or the delay if longer, power-off 4 s; repeat
  8→30/s, 4→10/s, 2/s flat). Firmware: `apply_settings` sets the UI
  task's mapper each loop; simulator: at each key edge
  (`Host::key_timing`).
* **Key confirmation sound**: 05 §3's key click already is the key-press
  sound; what longer holds add is not knowing when a hold registered, so
  Key click became Off / Presses / Presses and holds (0/1 keep their
  meaning in stores): `System::key_hold_click` → `services::hold_click`
  (same microphone/radio rules) → `audio::click` in the firmware.
* **Test safety**: the firmware's test look reads `0x1001–0x1003` as
  their defaults until a session sets one (`A11Y_ARMED`, RAM), so the
  suite, fuzzers and soak keep their key timing and shots whatever the
  owner chose; the session whitelist (firmware and xtask) gains them.
* **Tests**: matrix rows `s1001–s1003`; `effects::{text_size_pages_by_the_
  rows_shown, hold_delay_moves_every_hold, key_repeat_slows_a_held_key,
  key_click_confirms_holds, reduced_motion_stops_blinks_and_spinners}`;
  `input.rs accessible_hold_and_repeat`, `font.rs sized_faces`, `misc.rs
  spinner_stands_still_with_reduced_motion`, `settings.rs
  accessibility_packed`, the two theme tests. Manifest: new entries
  `0x1001–0x1003`, `0x0D60`; Key click, Theme, Animations updated; new
  context `Settings.Accessibility` (nine status-bar settings declared on
  it). Goldens: 63 `a11y_*` (Home, Home playing, Files, its menu, Settings,
  Display, Now Playing, Nothing playing, a dialog × large and extra large
  in Latin/Cyrillic/Japanese, both contrast themes, contrast + large),
  `app_group12_*`, Theme choice and Settings root (scroll thumb) changed —
  all looked at. Journey `settings_accessibility` (hold delay 2 s: 0.7 s
  Menu = menu, 2.3 s = quick panel; Large Settings shot; hold clicks ≥ 3
  of 4 via `status clicks=`), expected frames written from the simulator;
  `settings/settings.png` changed (scroll thumb).
* **Firmware** (before → after): resident 81,155 → 81,187 B (+32:
  mergeable constants); `.bss` 125,408 → 125,448 B (+40: a settings cache
  slot, the 14th row of the list screen in the UI's static, a byte each
  in the shell and the dialog); stack nested worst case 15,152 → 14,896 B,
  usable 19,712 → 19,200 B (`__stack_bottom` 0x132730: two 256 B steps,
  .data/.bss moved), **margin 4,560 → 4,304 B** (budget 4,096). The 14th
  row (13 groups + title) first cost the UI task 1,256 B (`new_ui` →
  `AppScreen::create` with `Rows::default()` inlined in every arm: margin
  3,048 B); `Rows::default` out of line brought it back. Rodata pages
  20 → 22 of 48 (paged rodata 160,922 → 176,009 B: BOOK16 14,468 B plus
  strings); code 121/128 pages (970,780 → 974,924 B). `check-resident`
  ok. **Not measured**: idle Home line loads (the shell reads the text
  size each draw, `apply_settings` the key timing each loop: two small
  settings lookups; was 615/s, limit 1,500).
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo clippy -p orpheus-apps --all-targets
  -D warnings`, `cargo test --workspace` (1,879 passed, 0 failed, 12
  ignored), `cargo xtask settings-matrix --check` (166 settings, 11,157/11,205 cells, 0 errors); firmware `fmt
  --check`, `clippy --workspace -D warnings`, `cargo xtask build`,
  `check-resident`, `size --check`, `stack --check`: pass. **Not run**:
  `xtask test all` beyond these, `brand`/`site --check`, every `xtask
  dev` step (no hardware).
* **Lead, on the player**: `cargo xtask dev suite settings_accessibility`
  (and the whole suite: the test look now covers the three settings);
  idle Home line loads; user check 31.
* **Open**: A11Y-1 (screen reader) will want the same group; reduced
  motion's "long names are spoken instead" is A11Y-1's; dialogs whose text
  needs more than four 16 px lines lose the rest to an ellipsis at large
  sizes; the Dark/Light contrast finding above.

### 2026-11-26 — SEQ-1: the Sequencer on the host and in the simulator (worker, no hardware)

New crate `crates/orpheus-seq` (`no_std`, no `alloc`, no `unsafe`, no
`core::fmt`, no floating point outside the synth) on `orpheus-synth`;
feature `seq` of `orpheus-apps` (on in the simulator and the browser
build, **off in the firmware**: no synth there until SYN-3). Full "as
built" in 05 §7.16; in short:

* **Model**: 16 patterns × 1–64 steps × 8 drum tracks; a step is 3 bytes
  (on, velocity, probability in sixteenths, ratchet 1–4, accent, a
  micro-timing *delay* 0–12/24 step); tracks with kit note, level, mute,
  solo; one kit (`Kind::Kit(0)`, one engine part: choke groups work across
  tracks); tempo 40–240 BPM (1/100 BPM), swing 50–75 %; play modes Pattern
  (switch at the pattern end) and Song (chain of ≤ 64 pattern × repeats).
  Synth tracks are SEQ-2.
* **Player**: exact step timing in integer units (a step pair = 57,600
  units; frame = anchor + ⌊units·rate·5/(tempo·96)⌋): every hit on the
  floor of its exact time at any tempo, swing, delay and ratchet, an hour
  without drift; tempo re-anchors at the next step; a step is read when it
  starts, so **every edit is heard from the next step** (the reason
  micro-timing only delays). Block-size independent; the audio equals the
  engine fed the same notes by hand; seeded probability, so a saved song
  renders bit for bit.
* **Screen-off API**: `Studio::render_i16(&mut Engine, &mut [i16])` is all
  the synth task needs; the UI edits the project between blocks and reads
  `Sequencer::position_at(dac_frame)` (the playhead from the audio clock).
* **File**: `/.orpheus/seq/SEQnn.OSQ`, `OSQ1` v1 records + CRC-32, the kit
  as an embedded `OSB1` bank; new project 383 B, the demo 2,167 B, at most
  25,203 B; older versions load (longer/shorter records, unknown tags),
  newer refused.
* **Screens**: project list, grid (full screen, 16 × 8 cells of 7 × 12
  px, playhead, header, page footer), transport (play, tempo, swing, tap
  tempo, mode, pattern, length, kit), step, track, mixer (mute/solo/level).
  On Home only with `set:seq=on` in the simulator (like Books), so no
  existing golden moved. Strings English (appended to the table).
* **RAM**: step 3 B, pattern 1,537 B, project 25,092 B, player 1,880 B,
  studio 27,528 B (+ the 62,320 B engine). On the player the project
  cannot live in `.bss` (≈ 1.5 KB left): with the synth in the decoder
  arena (62.3 of 72 KiB), the project fits the idle 32 KiB stream ring
  (no music plays while the synth does) — SYN-3/SEQ firmware decision.
* **Settings**: none added. The Home-screen mask's default is now 16383
  (14 apps with the feature; manifest updated); new matrix contexts
  `Sequencer` (list) and `SeqGrid` (grid), placed as Books and the
  reader are (Theme and Accent change the grid; status-bar settings the
  list): `settings-matrix --check` 11,090 / 11,138 cells, 0 errors.
* **Tests**: `orpheus-seq` (lib 3, `timing` 6, `play` 18, `file` 7, fuzz
  corpus 2; `orpheus-apps` 1 for the tempo text): timing exactness, swing, micro/ratchet, an hour, tempo at the
  next step, hand-placed audio, block sizes, chaining (song with repeats,
  empty song, pattern switch at the end), edits/mute/solo/swing/length at
  the next step, levels/accents, seeded probability, the densest pattern
  (no hit dropped), saved song bit-identical, screen-off render and
  playhead, stop/play, kits, every factory kit on every track, clamping,
  RAM; round trips (demo, 40 random projects), sizes, every truncation and
  byte flip refused, newer version, impossible records, forward
  compatibility, names. Fuzz target `seq_load` (`crates/orpheus-seq/fuzz`,
  10 seeds in `tests/fuzz-corpus/seq_load/`): 252,331 runs in 150 s with
  `nightly-2026-11-05` under ASan, no finding. Simulator: 38
  goldens `seq_*` (looked at), journeys `sim/tests/seq.rs` (8: make a beat
  by keys, hear it, save, restart, open; grid keys; mixer mute/solo while
  playing; transport incl. tap tempo; step editing; track sound/level;
  12 s with the screen off: frames and steps advance; delete and a failed
  save).
* **WAV for the owner** (made through the model: `cargo run --release -p
  orpheus-seq --example render -- <dir>`): `night-bus.wav` ("Night Bus",
  98 BPM, swing 56 %, punch kit, A A B B A C B D, 24 s, 220 hits, peak
  −2.8 dBFS) and its project file `SEQ01.OSQ` in the session scratchpad
  `seq1/` (not committed).
* **Gate**: `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -D warnings`; `cargo clippy -p orpheus-apps --all-targets
  -D warnings` with no features, `seq`, `seq,games`, `seq,books`,
  `games,salvo,bricks` and all; `cargo test --workspace` (1,910 passed,
  0 failed, 13 ignored; was 1,865); `cargo xtask settings-matrix --check`
  (above); `cargo xtask build`, `size --check`, `check-resident`,
  `stack --check`: pass. **Firmware** (the `seq` feature is off there;
  only the 40 appended strings reach it): resident 81,155 → 81,203 B
  (+48: code generation of the rebuilt crates), `.bss` 125,408 B
  unchanged, `.rodata` +648 B (the strings) moved `__stack_bottom` by
  688 B across one 256 B step: stack usable 19,456 → 19,200 B, **margin
  4,560 → 4,304 B** (budget 4,096). **Not run**: `xtask test all` beyond
  these, `brand`/`site --check`, every `xtask dev` step (no hardware;
  the brief forbids `xtask dev`).
* **User check**: listen to `night-bus.wav` (swing feel, ratchets and the
  fill, the levels of the punch kit). In the browser build/simulator:
  `set:seq=on`, make a beat with the keys.
* **For SEQ-2**: synth tracks (2 × monophonic/4-voice with note per step:
  a second step type with a note byte, the file's pattern record gains a
  track kind), sound screens and parameter locks (per-step locks need
  storage: a sparse lock pool per pattern), note entry/scale lock, live
  record, song editing screen, WAV export (`Studio` render offline is
  ready) and MIDI export (`Sequencer::run_dry` gives every hit with its
  frame), fill while held, count-in/metronome and the Sequencer settings
  of 05 §7.16 (each with VER-13 entries), early micro-timing (needs a
  half-step lookahead), more kits per project (2 engine kit slots).
* **For SYN-3 (and the firmware Sequencer)**: the engine (62 KB) in the
  decoder arena, the 25 KB project in the idle stream ring (or a smaller
  project), `Studio::render_i16` from the synth task into the DAC ring,
  the UI reading `position_at(clock − ring latency)`; a lock (BASEPRI
  critical section) around project edits vs a render block, or edits sent
  as messages; card I/O of `SEQnn.OSQ` in the card task (≤ 25,203 B file:
  read in pieces or into the ring); the clock floor High for drum patterns
  (SYN-2: 48–77 MHz + reverb); turn `seq` on in the firmware and put the
  app on the player's Home (`App::on_player`).

### 2026-11-26 — settings rounds 2 and 3, MIDI-1, SYN-2, BOOK-2 merged; screen lock vs tests (lead)

* Merged SYN-2 (`f7b756c9`), BOOK-2 (`65cb2602`), the Sound/Playback
  settings (`4c3d51a6`), the Display/Power/Time/USB settings (`69333b38`,
  conflicts in the Shell trait, `HostServices`, the firmware services and
  the journeys: both sides kept, three blocks closed by hand) and MIDI-1
  (`25d338d6`). 1,865 host tests; `settings-matrix --check`: 10,786 /
  10,834 cells (99.6 %), **4 settings without an effect** (Speed:
  WSOLA not built; Brightness: U-PWM-3; Wallpaper: Pictures › Set as
  wallpaper not built; Monitor: not in the firmware), 52 exclusions, 0
  errors. fw: resident 81,155 B, `.bss` 125,408 B, stack margin 4,560 B.
* **Found on the player:** with Backlight timeout and Lock delay in
  effect, the player locks itself ≈ 20 s after a reboot (defaults), and
  the suite stopped at its first journey ("the player's keys are locked:
  unlocking needs the Power key"); `ui test-look on` held the timers but
  left the lock. Fix (`b26c29e8`): `ui test-look on` wakes the screen and
  frees the keys (`KeyGate::wake_for_test`,
  `regress_test_look_wakes_a_locked_player`). On the player: locked=1 →
  test-look on → locked=0 screen=1.
* Suite on `b26c29e8`: **21/21** (new: `settings_sound`,
  `settings_playback`, `settings_screen_timers`, `settings_screen_saver`),
  idle Home 615 line loads/s, stack free ≥ 6,800 B; the owner's stored
  settings unchanged (`settings changed` before/after).
* Owner decisions pending: Speed (take it off Now Playing's menu until the
  time stretch, or label it); Brightness needs U-PWM-3 (stock duty
  values) from the `DRV_LCD.DRV` disassembly; Developer channel off/on
  only with the owner present.

### 2026-11-26 — VER-13 second round: six settings get their effect (worker, no hardware)

* **Key click** (0x0108): `orpheus_ui::shell::System::key_click` on every
  key down the gate passes; `orpheus_apps::services::key_click` (setting
  on, microphone not live); `orpheus_dsp::click` (3 ms 2.5 kHz triangle,
  linear decay, ≈ −15 dBFS, a function of its frame index: no table, no
  state but the index). Firmware: `audio::CLICK_AT` (the click's frame);
  the playback loop mixes it into each piece after the chain while it
  sounds (`click_mix`, out of line); with nothing playing or paused the
  card task sounds it alone (`click_alone`, woken by `Req::Alert`; dropped
  when card work waits; not over the radio). Simulator: `sys.clicks`, new
  service context `key-click`.
* **Fade on pause** (0x0109): `Chain::begin_pause`/`end_pause`
  (`PAUSE_FADE_MS` 60). Firmware: a pause while the DAC runs sets
  `Blk::fading`, the loop decodes on until the fade is done, `pause` lets
  the faded end play out (`finish`) and stops the DAC; playing again fades
  in (`resumed`, out of `play_block`); a fresh track starts at full level.
  Browser build: the engine fades out over 60 ms on a pause.
* **Gapless** (0x0203): `orpheus_player::Command::Gapless` — off, no
  `Prefetch`, every track change at a track's end is `Open { gapless:
  false }` (firmware: ring played out, DAC stopped, fresh open).
  Simulator: 200 ms of silence (`media::GAP_MS`); the `playback` service
  probe now includes a track's end in the middle of the queue.
* **Resume at start-up** (0x0204): after the state is loaded and the UI has
  published (≤ 2 s), `Server::resume_at_start` opens the saved place
  paused. Simulator: `set:resume=1` is a start-up with a resume point (on:
  loaded paused at 83 s; the `music_resume` golden now sets 0x0204=0).
* **Seek step** (0x0205): `seek_step_ms` — the setting is the first rung
  of the 2/5/10/30 s ladder (default 5 s: the first held steps are now 5
  s, were 2 s).
* **Auto-bookmark long files** (0x0207): `orpheus_player::AutoMarks` (three
  places of files over 20 min; noted at pause, leaving, every 30 s of play,
  shutdown; continued on `Opened` at frame 0 by a `Seek`; forgotten at the
  end). `player.bin` 588 → 680 B (key `AutoMarks` at 588, magic `YAM1`; a
  588-byte file loads without them). RAM paid by packing `Bookmark` to 28
  B (`repr(packed(4))`): `Player` stays 752 B. Firmware: the `Opened`
  flow (a seek) is carried out now (it was dropped). Simulator: the
  podcasts are 75 min long.
* **Speed** (0x0206): not implemented. 04 §5.1 defines it as a pitch-kept
  WSOLA time stretch (4 KiB of state, < 15 % CPU, the last step of
  WP-AU-6); a resampling rate change would alter the pitch. The 4 KiB have
  no home (.bss is 104 B from the next 256 B stack step, the arena's spare
  is 2 KiB and Opus leaves < 4 KiB), and 2× doubles the decode rate.
  **Proposal (owner/lead)**: take Speed off Now Playing's menu until the
  time-stretch package, or label it "not built"; its entry stays `gap`.
* **Observables** (`status`, read-only): `fade=` (last pause's fade, ms:
  60, 0, `-` none yet), `gapless=` (last track change at a track's end:
  1 gapless, 0 fresh start, `-`), `clicks=` (key clicks sounded). `SimPlayer`
  answers them. Journeys (RAM-only session, set back at the end):
  `settings_sound` (clicks on Home with Key click on; pause on Now Playing
  with Fade off → `fade=0`, on → `fade=60`), `settings_playback` (Repeat
  All, Gapless off, 30 s step: Next held ends the 30 s fixture →
  `gapless=0`; Gapless on, 10 s step, the track plays out → `gapless=1`;
  ≈ 35 s).
* **Matrix**: 22 → 16 settings without an effect, 69 → 63 exclusions,
  9,927 / 9,974 cells; dedicated tests in `effects` (seek step held,
  resume at start-up, gapless silence, podcast bookmark, clicks with the
  recorder, fade reported).
* **Firmware** (before → after): resident 81,076 → 81,140 B (budget
  81,920), .bss 125,280 → 125,344 B (budget 126,976; `__stack_bottom`
  0x132698, 104 B below the next 256 B step), stack nested worst case
  15,216 → 15,224 B of 19,456 B usable, margin 4,240 → 4,232 B (budget
  4,096; RT task frame 8,168 → 8,176 B). `play-block` cluster 14,830 →
  15,258 B, still 15 lines (the resume/fade code out of line to keep it
  there; 102 B left in its last line).
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo clippy -p orpheus-apps --all-targets
  -D warnings`, `cargo test --workspace` (1,728 passed, 0 failed, 11
  ignored), `cargo xtask settings-matrix
  --check`: pass. Firmware `cargo fmt --all --check`, `cargo clippy
  --workspace -D warnings`, `cargo xtask build`, `check-resident`, `size
  --check`, `stack --check`: pass. **Not run**: `xtask test all` beyond
  these (fuzz smoke, codecs), `brand`/`site --check`, every `xtask dev`
  step. `orpheus-fs` `bigdir` failed once at its first `create_dir` and
  passed when run again (another worktree's run in the same scratch
  folder, presumably).
* **Lead, on the player** (nothing run here): (1) `cargo xtask dev suite
  settings_sound settings_playback` (and the whole suite: `music`
  journeys start after a reboot, which now comes up paused on the last
  track — the suite's `stop` clears it); (2) listen: a pause fades over 60
  ms, playing again fades in; Key click on (Settings › Sound) clicks on
  Home and over music, not with the recorder open; Gapless off leaves a
  short gap between album tracks; (3) restart the player while a track is
  paused: it comes up paused at that place (Home's strip), OK plays on;
  with Resume at start-up off it does not; (4) a podcast over 20 min:
  leave it at 10 min, play something else, play it again → it continues;
  (5) the soak (`clicks`/fades on the playback loop: underruns stay 0).
* **Open**: the fade around seek and skip (05 §7.1 "fade on pause/skip")
  is not built; Resume at start-up always opens paused (the record's
  "was playing" is ignored: user power-off and battery cut-off are not
  told apart); while a track is open paused the library build and
  directory indexes wait (as during any pause) — after a restart that is
  now the usual state; Speed (above).
### 2026-11-26 — VER-13: Display, Power, Date & time and USB settings have their effect (worker, no hardware)

Owner's "100 % setting → effect": the settings VER-13 found changing only
their own row, except Sound/Playback (another worker). Each with host
logic and unit tests, simulator support and matrix entries, firmware
wiring; `effects.toml` entries without `gap` (12 of 14 settings).

* **Backlight timeout 0x0302, Lock delay 0x0303** (05 §6.3):
  `orpheus_ui::input::ScreenTimer` + `Timeouts` (`services::screen_timeouts`):
  the screen goes off that long after the last key (the next key only
  wakes it), the keys lock Lock delay later (Immediately/5/30/60 s/Never);
  firmware UI loop (`apply_settings`), simulator `Sim::advance`. Tests:
  `input.rs` `screen_timer_*`, `setting_effects::screen_timer_values`,
  matrix `effects::backlight_timeout_turns_the_screen_off`,
  `lock_delay_locks_after_the_screen_went_off`. **Test safety**: under the
  test look (suite, fuzzers, soak) or a host test the timers are held
  (`services::TimerHold`); a session's `test-set` of 0x0302/0x0303/0x0307
  off its default arms them, back at the defaults they are held again and
  the screen comes on with the keys free (`session_set_hooks`). The
  simulator holds them by default too (`set:timers=run` runs them), so no
  existing test or golden changed. Device check: journey
  `settings_screen_timers` (`ui top` screen=0, then locked=1, then back).
* **Screen saver 0x0307**: Clock keeps the panel lit at the timeout and
  the shell draws the time in dim 28 px numerals on a dark background
  (light themes too), moved every minute (`shell::saver_spot`); Power is
  always dark; `ui top` gains `saver=`. Goldens `app_screen_saver_*` (6).
  Journey `settings_screen_saver` (turns the look off for its duration, the
  look reads 0x0307 as its default).
* **Animations 0x0308**: `System::animations` → `Gfx::motion`: list and
  mini-player marquees at rest when off (radio's presets list in the
  matrix). The 120 ms screen slide of 05 §4 is not built, so marquees are
  all there is to stop.
* **Rotate 0x0309**: 180° in software (no controller flag is a hardware
  fact yet): `gfx::turn_band`/`rotated_band` — each flushed band turned in
  place, sent to the mirrored panel rows (`Display::flush_band`) and turned
  back, so the frame buffer stays upright for drawing and screenshots; the
  key mapper's existing `rotated` swap set from the setting. Simulator
  `Sim::panel`; the matrix compares the panel (every screen + the radio and
  recorder journeys, whose keys land elsewhere). `sim/tests/apps.rs`
  `walk_every_settings_group` turns Rotate back off after toggling it.
* **Power mode 0x0401**: `governor::PowerMode` + `Governor::step_in`
  (Battery saver: Base or the audio floor, a higher level left at once;
  High performance: ≥ High while the screen is on; Automatic = `step`);
  Battery saver also stops the marquees. Firmware `ui::POWER_MODE` → the
  watchdog task's governor. Test `power_mode_setting_values`. The backlight
  cap of 02 §5.4 waits for brightness levels.
* **Keys when locked 0x0404**: `KeyGate::when_locked` from the setting;
  new `Gate::Transport`: OK/Next/Prev while locked go to playback
  (`services::locked_transport`: play/pause, next, previous track), never
  to the screen under the lock (before, a pass-through would have reached
  the hidden screen). Tests `keys_when_locked_setting`,
  `effects::keys_when_locked_reach_volume_and_transport`.
* **Time zone 0x0505**: the RTC keeps local time; UTC = local − offset
  (`services::utc_of_local`; firmware `vault::unix_now`, simulator
  `unix_time`): the Authenticator's codes and the typed code follow it.
* **Correction 0x0506**: `services::ClockDrift` steps the running clock a
  whole second at a time (+60 s/day: one second every 24 min); firmware
  through `set_now`, never during a test session.
* **Set from RDS 0x0507**: `orpheus_fm::rds` decodes group 4A (MJD,
  hour, minute) and takes it once the next minute's group agrees (C/D
  carry no error report); `Rds` stays 112 B (counters to u16) because the
  SRAM3 shared window is full (`rec::Shared` hit its end with +8 B). The
  fake tuner sends CT (`chip.clock`, `set:rdsclock=UNIX`). Firmware
  `Fw::rds_clock` sets the RTC in the time zone, never during a session.
* **Read-only card 0x0902**: the UI's File transfer sets `MSC_READ_ONLY`
  from it (SCSI WP and refused writes already in `orpheus-usb`); the USB
  question shows a lock on the File transfer row (goldens
  `app_usb_read_only_*`, 6).
* **Developer channel 0x0903** (06 §2.5 as built): off, the developer
  personality enumerates with no interface, the others without the vendor
  interface, and no vendor request is answered (`usbdev::builder`). Held
  on while the test mailbox, the test look or a settings session is
  active; `settings test-set` refuses it (host test
  `session_refuses_the_developer_channel`; the SimPlayer now refuses what
  the firmware refuses). A change re-enumerates the developer personality.
  Default on: the build behaves as before (one atomic load per build of
  the device). Ways back: Settings › USB › Developer channel on (keys),
  Factory reset; recovery (ADFU) is unchanged (Settings › System ›
  Recovery mode, power menu, reset + Next) — `docs/user/troubleshooting.md`.
* **Left (reported)**: **Brightness 0x0301** — missing hardware fact: the
  stock firmware's brightness duty values for PWM4 (and U-PWM-1); new
  unknown **U-PWM-3** (01 §9). **Wallpaper 0x0306** — the spec defines it
  only as a picture from Pictures › Set as wallpaper (not built, AU-14); no
  other values are specified.
* **Fixed (tooling)**: a journey's `cmd: ui top ~ …` could never pass
  (`player::command` primed the reply with `ui top` itself):
  `regress_ui_top_check_was_primed_with_itself`.
* **Stack**: the first build had the UI task's poll frame 848 → 1,168 B
  (margin 3,920 B, below the 4,096 B budget): `Shell::key` inlined into
  it. `#[inline(never)]` on `Shell::key`: frame 520 B.
* **Measured** (firmware, `cargo xtask build/size/stack`): resident
  81,075 → 81,091 B; .bss 125,280 → 125,352 B (+72: the screen timer,
  drift, statics; the stack start did not move a 256 B step); static stack
  margin 4,240 → 4,568 B (thread context 5,880 → 5,552 B);
  `check-resident` ok.
* **Matrix** (`settings-matrix --check`): 10 settings without an effect
  (was 22; the rest are Sound/Playback's and Monitor), 0 errors.
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo clippy -p orpheus-apps --all-targets
  -D warnings` (no features), `cargo test --workspace` (1,738 passed, 0
  failed, 11 ignored; was 1,712), `cargo xtask settings-matrix --check`
  (162 settings, 10,212 / 10,260 cells, 10 without an effect, 58
  exclusions, 0 errors); firmware `fmt --check`, `clippy --workspace -D
  warnings`, `cargo xtask build`, `check-resident`, `size --check`,
  `stack --check`: pass. **Not run**: `xtask test all` beyond these
  (fuzz smoke, codecs), `brand`/`site --check`, every `xtask dev` step
  (no hardware); idle-Home line loads not measured (the UI loop now runs
  two small functions per pass: `apply_settings`, `publish_settings`).
* **Open / lead**: run the suite (two new journeys: `settings_screen_timers`
  ≈ 25 s, `settings_screen_saver`); user check 30 (USER-CHECKS.md); the
  `na` review texts of the other global settings still say the backlight,
  keys, clock and governor take no setting — now 0x0302/0x0307, 0x0303/
  0x0404, 0x0506/0x0507 and 0x0401 do (left untouched to keep this
  package's manifest edits to its own entries; a one-line sed after the
  Sound/Playback merge). Power mode on the player (governor statistics)
  has no journey: not in the session whitelist.
### 2026-11-25 — MIDI-1: MIDI files on the host and in the simulator (worker, no hardware)

New crate `crates/orpheus-midi` (`no_std`, no `alloc`, no `unsafe`, no
`core::fmt`), on `orpheus-synth`. Not in the firmware (SYN-3). The full
"as built" is in 04 §9; in short:

**Parser** (`smf`): formats 0, 1 (merged by tick, ties by track) and 2
(patterns one after the other, tempo reset at each); running status;
tempo, time and key signature, the seven text kinds (48 bytes kept),
end of track; SysEx and other metas skipped by length; SMPTE 24/25/29.97/30;
`RIFF RMID`. **Bounded streaming**: ≤ 64 track cursors of 64-byte windows
(≈ 97 B each, 6.2 KB in all); a call that needs bytes answers
`Need { offset, len }` and consumes nothing, the caller `supply`s (an
`async` card task can answer; `MidiSource` + `SliceSource` drive it
synchronously); each `next_event` parses ≤ one delta per track and one
event. **Adversarial**: no length trusted (chunks clamped to the file,
≤ 320 chunks walked, header track count ignored), > 64 tracks refused
(`TooManyTracks`, "MIDI file has more than 64 tracks"), 5-byte numbers,
stray data bytes, undefined status bytes end only their track, a missing
end of track is synthesised, 64-bit saturating ticks, songs cut at 24 h.

**Timing** (`timing`): frame = ⌊(acc + Δtick·µs·rate) / (tpq·10⁶)⌋ with the
exact accumulator carried across tempo changes (128-bit): every event is on
the floor of its exact time, however many tempo changes. A first version
re-anchored on the floored frame and drifted up to a frame per change
(`regress_tempo_changes_drifted_by_a_frame_each`, caught by the test before
commit). `TempoMap` (128 points) for tick/BPM views.

**Sequencer** (`seq`): part = channel; volume·expression with the GM
40 log₁₀ curve, pan, sustain, modulation and channel pressure as vibrato on
top of the patch's, RPN 0 bend range (patches installed with ±24, `Bend`
scaled), reverb/chorus sends, reset all controllers (RP-015), all notes /
sound off; program changes install a sound only when it changes; channel
10 → kit slot 0, its program picks the factory kit. State can be scanned
silently and `restore`d at once (seek and start share it).

**GM sound set** (`gm`): 32 sounds of our own for 128 programs (table in
04 §9 and the module doc): 9 reuse SYN presets (5 with levels adjusted),
23 new (piano FM 3-op, harpsichord, bell, mallet, church and reed organs,
nylon/steel/drive guitars, bass, slap bass, solo strings, harp, timpani,
horn, sax, reed, square lead, warm pad, noise fx, chirp). Levels balanced
by the tour's per-bar RMS (−22 … −30 dBFS, the noise swell −33). Written as
an `OSB1` bank (`data/gm-sounds.osb`, 7,312 B: 32 patches + the 5 factory
kits; a test keeps it current and decodes it back bit for bit).

**Player** (`MidiPlayer`, ≈ 70 KB): open = header + tracks + a silent scan
(duration = last event + 2 s, channels, notes, title from `@T` or track
0's name, lyric mode) in bounded steps; `render`/`render_f32` (blocks cut
at program changes and a full 32-event buffer; any block size gives the
same samples); `seek` = rewind + silent scan to the target in ≤ budget
events per `step`, `Engine::reset_at(seed, target)` (new: the control grid
stays on the samples of a render from the start), state `restore`d, held
melodic notes re-struck; pause = not calling; `set_mute`/`set_solo`;
lyric pieces queued with their frames (`pop_lyric`); `summary()` for
duration, title, tempo map and lyrics without an engine. Master gain 0.5:
a fff six-note chord on any program stays below full scale (one program,
tuba, touches the limiter at −0.5 dB).

**Simulator**: `orpheus-apps` feature `midi` (on in the simulator only)
makes `.mid`, `.midi`, `.kar`, `.rmi` audio in Files and the library; the
simulated player takes duration and title from the scan and turns `.kar`
lyrics into timed Now Playing lines; `set:queue=midi` writes the three demo
songs to `/Music/MIDI` (the built card is unchanged, so no other golden
moved). New goldens `media_np_kar_lyrics_*` (6; looked at: the current
line "the river runs, the air is still." highlighted at 0:08.2), journey
test `midi_files_play_from_files`.

**Tests** (38 in the crate): parser (16: formats, running status, metas,
SysEx, long and empty texts, SMPTE incl. 29.97, RMID, header errors, too
many tracks, untrusted lengths, every truncation of a file, huge deltas,
corrupt bytes, bounded steps); playback (16: tempo changes exact to the
sample — event frames against an independent rational computation at
44.1/48 kHz and the render bit-identical to the same notes placed by hand;
golden hashes of 4 renders, also in a release build `--check`; block-size
independence; GM sweep of 128 programs × extremes + chords and 5 kits ×
47 notes: finite, below full scale, audible; seek bit-exact into a silence
at frame offsets 0/17/31, into a phrase (held notes re-struck) bit-exact
from the next phrase, a real song within 1 dB without drums / 3 dB with;
bounded seek steps; `.kar` and lyric-event timing exact, the player's
queue equal to the summary; mute/solo; duration and position; programs and
kits; reopen; RAM ≤ 72 KiB; the bank); fuzz target `midi_parse` (parser,
summary, player with odd blocks and seeks; 11 seeds in
`tests/fuzz-corpus/midi_parse/`) run 120 s with `nightly-2026-11-05`
under ASan: 70,882 runs, no finding.

**RAM**: player 70,224 B (engine 62,320, parser 6,216, sequencer 376,
event buffer, lyric queue) — fits the 72 KiB decoder arena, asserted.
**Code** (thumbv7em, opt "s", no LTO): the play path ≈ 10 KB (`Smf::next_event`
1.35 KB, render 1.0 KB, sequencer 2.3 KB, clock 0.8 KB, restore/step/new
2.4 KB); the GM table 9.6 KB read-only; the rest of the crate's 39 KB is
the demo writer and its sort (not linked by a player). 0 fused
multiply-add instructions.

**CPU estimate** (`cargo run --release -p orpheus-midi --example
estimate`: the demos' real voice mix, per voice SYN-1/2's disassembly
cycles by sound, ±30 %; engine + limiter 135 and reverb 290 per sample):
the etude (mean 10.6 voices, max 20 with tails) ≈ 2,430 cycles/sample
mean: **≈ 107 MHz at 44.1 kHz, 117 MHz at 48 kHz, worst block ≈ 186/203
MHz**; the karaoke song (5.5 voices) ≈ 60/65 MHz; parser + clock ≈ 22–66
ns per event on the host (a few thousand M4 cycles at most; < 1 MHz at
hundreds of events per second). **A typical 8–12-voice GM file needs the
High clock (144 MHz)**; full 16-voice peaks need Turbo (192) or Max (288,
D-4), or a voice limit of ≈ 12 at High.

Gate: fmt, clippy `-D warnings` (workspace, all targets), `cargo test
--workspace` (1,822 passed, 12 ignored, 0 failed): pass; release `render
--check` 4 scenarios, 0 mismatches; firmware unchanged (the `midi` feature
is off there): `xtask build` ok (resident 81,076 B), `xtask size --check`
within budgets. No settings added (settings matrix not affected). Not run:
HIL (no hardware), `xtask test all` beyond the steps listed.

**User check**: listen to the three renders (etude, karaoke song, sound-set
tour) — balance between the 32 sounds and the kits, and whether the piano
(FM, 3 operators) is acceptable as the most used program.

**Open / for SYN-3**: (1) a card source for the parser's `Need`s (64-byte
reads at arbitrary offsets: a sector cache in front, ≤ 64 tracks); (2)
the player value (70 KB) in the decoder arena; `render` into the DAC ring;
(3) the clock floor High while MIDI plays, a voice limit or Max for the
peaks (estimate above, to be measured); (4) the `midi` feature on in the
firmware (Files, library) once it plays; Now Playing lyrics from
`pop_lyric`; a mute/solo and tempo view; (5) SF2: not done (stretch; the
sample cache and `Kind::Sample` are the way in; SF2 parsing, zones and
modulators are a package of their own).

### 2026-11-26 — VER-13 merged; `settings_shuffle` on the player (lead)

* Merged VER-13 (`6a2dff67`): `cargo xtask settings-matrix --check`
  (162 settings, 9,365/9,412 cells, 99.5 %, 69 reviewed exclusions, 0
  errors); 1,712 host tests.
* The new journey `settings_shuffle` on the player: `status shuf=` follows
  Shuffle 0 → 1 → 0 through the RAM-only session (0 underruns); its shot
  failed (the simulator kept a now-playing strip after `stop` that the
  player clears, and before that Home's label strip): the Playback group
  is now shot first, before anything plays. Passes on the player; the
  owner's stored settings unchanged (`settings changed` before and after:
  `d50=9500`).
* Owner decisions from the VER-13 findings (not changed): 22 settings
  have no effect yet (list in the VER-13 entry); ReplayGain Auto = Album;
  Tag code page only distinguishes Cyrillic; Output is never read by the
  firmware; games and Books strings are English only; Files › Sort is not
  applied to folder queues in the simulator (07 §5.12 asks for it).

### 2026-11-26 — VER-13: setting → effect coverage, tool and gaps (worker, no hardware)

* **Tool**: `cargo xtask settings-matrix [--check] [--strict] [--manifest
  PATH]` (07 §5.12 "As built"; alias `settings coverage`). Settings from
  `orpheus_apps::settings::all()` with every feature (162 with a value:
  98 of the groups and apps, 58 games' fields and words, 8 reading
  settings and word); the manifest parsed as TOML with unknown fields
  refused; entries, values against the kind, the code's default,
  contexts (declared, `@groups`), global accounting (`na`), `cover` per
  context, `uncovered`/`gap` as reviewed exclusions, tests resolved to a
  non-ignored `#[test]` by file and module path, `suite:` journeys,
  `[kinds]` persistence tests. `--check` in `xtask test all`;
  `--strict` (no exclusion) in `xtask gate --m11`. Unit tests (source
  parser, complete manifest, uncovered/absence, global accounting,
  values/defaults/kinds/checks, gaps) and
  `regress_settings_matrix_missing_entry_and_dangling_test`.
* **Before**: 64 of 162 settings had an entry (the games' and Books', all
  local), none with contexts the tool could check (0 cells; 358 errors:
  "Games", "GamePause" … undeclared, no `unaffected`, no cover); 98
  settings (all of Sound, Playback, Display, Power, Date & time,
  Bluetooth, Radio, Recording, USB, Language, System and the apps' own)
  had none. **After**: 162 entries, 0 errors; global 31 settings (10 full,
  8 partial, 13 gap; cells 1,261 / 1,269), local 125 (89 full, 27
  partial, 9 gap; cells 8,096 / 8,135), storage 6 (full); matrix 9,365 /
  9,412 cells covered (99.5 %); 69 reviewed exclusions (22 settings
  without an effect, 47 contexts without a test). 23 settings carry
  `device = true` (1 fully covered on the player: Shuffle).
* **Simulator matrix** (`sim/tests/settings_matrix.rs`, reads the
  manifest; 163 tests, 7 s): each non-default value against the default
  in 59 screen contexts (frame) and 10 service contexts (`playback` queue
  and its end, `radio-tuner` register writes, `recording` file,
  `usb-keyboard` reports, `usb-host` personality, `bt-module`,
  `steps-counter`, `alarms`, `sleep`, `volume`): different exactly where
  declared, equal elsewhere. Mutation check: moving Date format's `Lock`
  context to `Home` fails with "declared context `Home` shows no effect"
  and "leaks into `Lock`". Dedicated tests: custom EQ bands with their
  preset, input gain with manual gain, auto-split minutes, the four
  alarms' time/days/on, the games' score-table variants after a finished
  Blocks, Solitaire and Salvo game (with a control: ghost, previews,
  timer leave the list alone).
* **Host**: `sim/web` sound (each Sound setting and custom band changes
  the rendered output through `Sound::from_settings` and the real chain;
  no other setting reaches the chain); `orpheus-power`
  `performance_setting_values`; `orpheus-steps` `sensitivity_setting_values`;
  `orpheus-rec` `split_ends_the_file_at_the_split_time`; `orpheus-apps`
  `persistence::{persist,default,clamp,migrate}_{toggle,choice,range,
  language,homeapps,packed}` (every setting of each kind: written and read
  back after a reboot with the others untouched, defaults and factory
  reset, out-of-range → the default, unknown keys / other record lengths
  / missing keys of an older store). `orpheus-hid` `FakeHost` keeps a
  digest of the reports (layout and keypad digits show).
* **Fixed (simulator)**: a queue made from a folder or the library
  ignored Settings › Playback › Shuffle (the firmware ORs it in):
  `regress_sim_queue_follows_the_shuffle_setting`.
* **Device**: journey `settings_shuffle` (`tests/suite/journeys.toml`,
  expected frame made through the test): stops unless `settings
  test-session` answers "on", then 0 → 1 → 0 through `settings test-set
  0x0201` with `status ~ shuf=N` after each, ends Off (RAM only; the
  suite's closing reboot restores the stored value). `SimPlayer` answers
  `settings test-session`, `settings test-set`, `status … shuf=`.
* **Findings (settings with no effect: `gap`)**: Key click 0x0108, Fade on
  pause 0x0109, Gapless 0x0203, Resume at start 0x0204, Seek step 0x0205,
  Speed 0x0206 (shown in Now Playing's menu only), Auto bookmark 0x0207,
  Brightness 0x0301 (backlight fixed at 12), Backlight timeout 0x0302,
  Lock delay 0x0303, Wallpaper 0x0306 (one value), Screen saver 0x0307,
  Animations 0x0308, Rotate 0x0309, Power mode 0x0401, Keys when locked
  0x0404 (KeyGate fixed to volume keys), UTC offset 0x0505, Correction
  0x0506, Set from RDS 0x0507, Monitor 0x0808 (not built), Read-only card
  0x0902, Developer channel 0x0903: the matrix shows each changes only its
  Settings row. Also: ReplayGain Auto = Album (mapping `2 | 3`); Tag code
  page has an effect only for Cyrillic (fw library.rs); Output 0x0604 is
  not read by the firmware; Language leaves the Games list, Books, the
  reader menu, Solitaire, 4096 and Salvo unchanged (their strings are
  English in every pack); Files › Sort is not applied to queues made from
  a folder in the simulator (07 §5.12 asks for it: open question).
* **Reviewed exclusions (47 contexts)**: the player's own sound path for
  the 16 Sound settings and bands (no read-only report of the applied
  Sound or an output CRC yet), the clock level (Performance), the ADC gain
  (Input gain), auto-off (rule 4; not in a host crate), background step
  counting, the library's code page, Output; the Games list and pause
  menu for 4096's and Bricks' variant settings (no scripted finished game)
  and Salvo's players 3–4 (pairwise with Players).
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo clippy -p orpheus-apps --all-targets
  -D warnings` (no features), `cargo test --workspace` (1,712 passed, 0
  failed, 11 ignored), `cargo xtask settings-matrix --check`: pass. Firmware: not changed (no
  fw build steps needed). **Not run**: `xtask test all` beyond these
  (fuzz smoke, codecs), `brand`/`site --check`, every `xtask dev` step.
* **Open / lead**: run the suite on the player for `settings_shuffle`
  (the `shuf=` after a test-set may need the 1 s wait); to close the
  device exclusions: a read-only `status` field with the published Sound
  and the clock level, `session_settable` extended to Radio (0x0701–3,
  config only), auto-off's rule into `orpheus-power`.
### 2026-11-25 — SYN-2: drums, kits, samples, effects, banks (worker, no hardware)

Host only; nothing in the firmware, simulator or apps (SYN-3, MIDI-1,
SEQ-1). The full "as built" is in 04 §9; in short:

**Drums**: 11 models with tune/decay/tone/snap/level, from published
descriptions of analogue circuits (frequencies and structure only):
SYN-1's six unchanged (their golden hashes kept), new rim, cowbell,
cymbal (two bands, ride → crash), clave/wood block, shaker. **Kits**:
`Kit` of ≤ 32 nine-byte `KitEntry`s (drum or sample hit, choke group,
pan, tune, decay, tone, snap, level), 2 kit slots in the engine,
`Kind::Kit(slot)`; choke groups symmetric within a part; factory kits
`standard` (GM, 32 notes), `analog`, `punch`, `tight`, `percussion`.
**Samples**: `SampleSource` (512-byte blocks of numbered files; the card
in SYN-3), `SampleCache` over borrowed memory (256-frame pages, pinned
heads, `service` streams what the play heads reach in the next 1024
frames, misses play silent and are counted), WAV PCM 8/16/24/32/float,
mono/stereo (to mono), extensible, `smpl` loops; 4-point Hermite, 32.32
position, bit-exact at unit speed; `Kind::Sample` through filter and
envelopes; kit sample hits. **Effects**: delay exact to the sample at
tempo up to 256 ms, to 1.02 s at a reduced line rate (filtered,
centre within a sample), feedback low cut; reverb now an 8-line FDN
(Hadamard) at half rate, 8 KiB instead of 16 KiB. **Banks**: `OSB1`
records of patches (`f32` bit for bit) and kits; 4 new patches
(acid-bass, strings, brass, sub-bass) and `SAMPLER`.

**Looked at, fixed** (spectrograms of every render; no listening here):
cowbell harmonics to 20 kHz → a 3.5 kHz low pass after the band pass;
images of long-delay repeats to 20 kHz → anti-alias/anti-image low
passes with their delay compensated; the reverb's truncation shortened
decays (0.8 s → 0.48 s; `regress_reverb_truncation_shortened_the_decay`)
→ rounding above a limit-cycle-free threshold; the full-rate FDN cost
≈ 900 M4 cycles/sample → half rate, block-wise lines, saturating casts
through `i32` (`as i16` costs three compares; `math::to_i16` too, same
values); sample voices freed their slot mid-segment (block-size
dependence) → end on ticks; the test pluck's loop clicked (decaying
tone) → a steady loop region.

**Measured (host, x86-64, opt "s")**, ns per voice-sample: kick 12.2,
snare 18.0, hats 23.6/24.9, clap 5.6, tom 10.4, rim 10.6, cowbell 9.9,
cymbal 24.2, clave 7.0, shaker 6.0, sample voice 7.4 (pitched + LP
10.6), kit sample hit 7.3; per sample: delay 5.1 (1 s: 10.9), chorus 9.9,
reverb 26.6. **Player estimate** (thumbv7em disassembly, ±30 %), cycles
per voice-sample: kick ≈ 145, snare ≈ 230, hats ≈ 295, clap ≈ 105, tom
≈ 155, rim ≈ 120, cowbell ≈ 155, cymbal ≈ 315, clave ≈ 100, shaker ≈ 75,
sample + LP ≈ 95, kit hit ≈ 90; delay ≈ 90 (long ≈ 230), chorus ≈ 120,
reverb ≈ 290. A kit groove (6–8 drum voices) ≈ 48–77 MHz + reverb ≈ 14
MHz at 48 kHz: drum patterns need the High clock.

**RAM** (host sizes): engine **62,320 B** (asserted ≤ 64 KiB; reverb 8,600,
delay 24,888, chorus 4,144, kits 2 × 289, voice slot 328 unchanged);
sample cache state 4,904 B + borrowed pages (512 B each). Code 64.3 KB,
read-only 26.7 KB (thumbv7em, opt "s", no LTO, whole crate).

**Tests**: 61 in the crate (27 unit, `drums_fx` 9, `engine` 10,
`samples` 7, `fuzz_corpus` 6, `golden` 1 of 45 scenarios, `spectrum` 1):
sample-accurate hits and sample notes, choke groups, delay timing at
tempo, reverb decay rate and exact silence, no NaN/subnormal/clipping
with every drum and effect at extremes, streaming equals pinned,
block-size independence, WAV formats and malformed files, bank round
trips; release `render --check` 49 scenarios, 0 mismatches. Fuzz
targets `wav_parse` and `bank_decode` (8 + 5 seeds in
`tests/fuzz-corpus/`), 90 s each with `nightly-2026-11-05` under ASan
(4.8 k and 69.6 k runs; no finding).

Gate: host fmt, clippy `-D warnings` (workspace, all targets), `cargo test
--workspace` (1,533 passed, 11 ignored): pass; thumbv7em build of the
crate: 0 fused multiply-add. Not run: `xtask test all` beyond `cargo test
--workspace`, firmware build, size/resident/stack checks (no firmware
change), HIL (no hardware).

**User check**: listen to the WAVs the worker wrote (kits, effects,
drums, samples; list in the hand-over) — the kits' balance and the
reverb's character are judged by ear only.

**Open / next**: SYN-3 (card `SampleSource`, cache pages in the stream
ring, `service` between blocks, measured cycles, clock floor for drum
patterns, hot loops clustered; whether drums should end at −60 dB);
MIDI-1 (GM programs → presets, channel 10 → `STANDARD`, SF2 through the
cache); SEQ-1 (patches and kits as bank records; 2 kit slots).
### 2026-11-25 — BOOK-2: EPUB (worker, no hardware)

Host and simulator; **not in the firmware** (as BOOK-1: feature
`orpheus-apps/books` off there). Design and as-built details: 05 §7.15
"As built (BOOK-2)".

* **`orpheus-books`** (`no_std`, no alloc, no `core::fmt`), new modules:
  `xml` (the tokenizer split out of FB2 and shared: attributes with
  references decoded and hashed, unquoted values, 204 named references),
  `zip` (end record, the central directory as a stream, local headers,
  CRC-32, path resolution), `inflate` (DEFLATE), `opf` (container, OPF,
  EPUB 3 navigation, NCX), `xhtml` (content documents → the normalised
  stream's events), `epub` (the job: `Epub::step` over a `Store` trait,
  one bounded piece per call; `Epub::library` reads title, author and
  language only). `convert`: `Format::Epub`, content documents as parts of
  one stream, `<meta>` charset switch, the anchor watch. `shelf`:
  `step_with(…, scratch)`, `needs_scratch()`, EPUB conversion and titles,
  `Book::waiting`, `Book::lang` (cache header bytes 24–27).
  `orpheus-apps`: the empty library's help names `.epub` (no other UI
  change: an EPUB opens, waits and reads with BOOK-1's screens).
* **RAM** (host `size_of`, `tests/ram.rs`): the EPUB job **2,880 B**
  (read buffer 512, three paths 3 × 257, a package document's reader 952
  or the converter 384, watch of 16 entries, inflate state 48, directory
  walk 72); **the scratch lent: 35,072 B** (32 KiB window + 2,304 B of
  Huffman tables). `Shelf` 9,360 → **12,160 B** (+2,360 B: the EPUB job in
  the card-work union, as the conversion and in the library search; +440
  B: files of a job left behind, a title to rewrite). Converter 356 →
  384 B, MetaReader 524 → 540 B.
* **The inflate window on the player.** 32 KiB static is out: SRAM0's
  `.bss` budget is full (124 KiB, measured 123,000 B) and the stack margin
  is ≈ 4.5 KB. Design as planned in 05 §7.15: the window and tables are a
  *lent* scratch — the card task's 64 KiB decoder arena while no track is
  loaded. Without it an EPUB waits ("Preparing", `Book::waiting`); the
  rest of Books (reading converted books, the library, layout) needs no
  scratch. Costs of the alternatives, for the integration to choose:
  (a) the codec arena's lendable remainder while music plays (35 KiB
  must be free beside the decoder: MP3's state would allow it if the arena
  lends a sub-slice; FLAC/Vorbis would not) — no card cost; (b) a
  card-backed window: inflated bytes appended to a 32 KiB ring file, a
  small RAM window (4 KiB) for near matches, far matches read back — one
  extra 512-byte write per 512 bytes inflated (≈ doubles the
  conversion's writes, all on one 64-sector file) and up to two sector
  reads per far match (a match is ≤ 258 bytes); several seconds per
  100 KB chapter at ≈ 1 ms a read, and the inflater would have to stop
  and resume at a far match (an `Error::Window`-like status) — not built.
  A smaller window alone (`Scratch::new` takes any power of two ≥ 1 KiB)
  decodes only streams that never reach further back, which zlib's output
  for books does.
* **Card work** (FAT32 with 512-byte clusters and exFAT images,
  `tests/shelf.rs`): ≤ 16 sectors read and ≤ 12 written in any step, as
  BOOK-1 (the library's title rewrite after a conversion is its own step:
  in the conversion's last step it made 15). In memory (`tests/epub.rs`):
  one book read of ≤ 512 B per step, ≤ 20 table writes; the EPUB 3 sample
  (6,833 B, 35,397 B of text) converts in 111 steps.
* **Tests**: `inflate` (known vectors: empty, stored, a hand-made fixed
  block, bad type, distance before the start, stored length check,
  truncation, over-subscribed code; every sample × level 0/1/6/9 × five
  piece/limit pairs against flate2 — a host dev-dependency only; 1 KiB
  window limits), `zip` (CRC check value, paths, end record incl. a
  self-extractor stub, a comment needing three reads back, zip64,
  multi-disk, not-zip), `xml`, `xhtml` (structure, soup, depth, meta
  charset), `opf` (container, package, navigation, NCX), `convert`
  (EPUB parts, the meta charset switch); `tests/epub.rs` (12, EPUB 2 and 3
  samples built from parts in `tests/common/epubkit.rs`: every paragraph
  and nothing else, contents and their positions, headings without
  navigation, contents out of order, library metadata, the scratch's
  rules, encrypted/unsupported/too large/bad CRC/truncated/garbage parts,
  bombs, refused archives, missing and odd pieces, KOI8-R/UTF-16/meta
  encodings, deep and messy markup); `tests/shelf.rs` EPUB on FAT32 and
  exFAT (library with and without the scratch, waiting, conversion,
  every page, contents jumps, chapters, cache on reopen, a broken EPUB,
  the title fixed on conversion, books unchanged, fsck clean); fuzz
  targets **`books_zip`** (8 seeds) and **`books_epub`** (13 seeds)
  replayed by `tests/fuzz_corpus.rs`; `sim/tests/books.rs` (+2: the EPUB's
  contents and pages, waiting without the scratch; rows after Moby-Dick
  moved by one); **12 new goldens** (`books_epub*`: first page, page 2,
  the list with italics, contents, the anchor jump, Medium; dark and
  light) and 11 changed (library lists, find-in: the EPUB's row), all
  looked at; the other Books goldens unchanged after the row remap.
* **Bugs found and fixed** (each with a `regress_` test that fails
  without its fix): **lost clusters when a conversion or layout was given
  up** (BOOK-1: opening another book during a conversion, or a new layout
  during one, dropped files without writing their directory entries;
  fsck.fat reclaimed 30 clusters in the test) —
  `regress_abandoned_job_loses_no_clusters`; FB2 `<body name="footnotes">`
  was not a notes body (the 8-byte buffer held `footnote`, compared with
  `footnot`) — `regress_fb2_footnotes_body`. During the package (new
  code): the watch cleared at a part's start (`regress_epub_watch_
  survives_part_start`), the directory walk skipping the extra field
  before the name (`regress_cd_walk_name_before_extra`), inflate output
  allowed past the window (`regress_inflate_max_over_window`).
* **Simulator**: `/Books/灯台守.epub` ("The Lighthouse Keeper", our own
  text: three chapters, nested navigation, an anchor, a list, a quote, an
  image; named to sort last in Files so the media goldens keep their
  rows), deflated with `miniz_oxide` (new sim dependency, pure Rust, in
  `Cargo.lock` already); `SimBooks` lends a 35 KiB scratch while the shelf
  asks; `set:bookscratch=off` lends none (as with music on the player).
* **For the firmware integration** (with BOOK-1's list: the Shelf split
  and its step futures, the chore pacing, rodata): (1) lend the decoder
  arena to `Shelf::step_with` while `needs_scratch()` and no track is
  loaded, unchanged from an entry's first step to its last (a track
  started in between: the next step without it restarts the conversion,
  `Step::Lost`; a title read falls back to the file name); the arena's
  first 35,072 bytes; (2) a "Pause music to prepare this book" hint for
  `Book::waiting` (a string; not added, as the other Books strings are
  English only); (3) the EPUB job (2.9 KB) is card-side state; its
  `Epub::step` future holds only small locals (≤ 48 B buffers) and the
  store's 512-byte reads go into its own buffer; the whole
  `Shelf::step_with` future measures 5,360 B on the host (64-bit; to be
  measured on the target with BOOK-1's step-future work); (4) rodata: the
  entity table (204 names), the CRC table (1 KiB), inflate's
  length/distance tables — not measured for the target.
* Open: images and their `alt` text, CSS (`display:none`, page breaks),
  footnote pop-ups, tables beyond a paragraph per cell, MathML,
  right-to-left and vertical text, DRM detection (encrypted parts read as
  `[damaged]` or garbage), on-device timing of inflate.
* Gate: `cargo fmt --all --check`; `cargo clippy --workspace
  --all-targets -- -D warnings` and `orpheus-apps` with features none,
  games, salvo, books, games+books, salvo+books: clean; `cargo test
  --workspace`: all pass (178 test binaries); `cargo xtask build`: ok
  (resident 81,076 B, firmware unchanged: `orpheus-books` is not in it);
  `cargo xtask size --check`: within budgets. Not run: `xtask test all`'s
  cargo-fuzz runs (nightly; the corpora replay on stable), brand, site,
  check-resident, stack, `dev hil smoke` (no firmware change, no
  hardware).

### 2026-11-26 — games-io and BOOK-1 merged; games during music clean (lead)

* Merged the `games-io` cluster (`606bb84a`: `cargo xtask hot`, a
  call-graph cluster of 14 lines with a `budget_lines` check in `stack
  --check`, `starts_over`, `CardRun::rare_step` out of line). On the
  player (`922ce7ac`): four game opens and quits with an MP3 playing,
  **0 underruns**, and no `serve:` line at all (every games piece under
  30 ms; before: 156–223 line loads and 30–53 ms per piece).
* Suite on `922ce7ac`: `games` failed on the player's saved games
  ("Continue" shortens "Solitaire" beside it); the values mask now starts
  at x = 42. Then 16/16.
* Merged BOOK-1 (`50eb115c`, Books in the simulator only): conflicts with
  GAME-6 resolved — Books screens renumbered 56–61 (Bricks keeps 55), the
  reading settings are list `0x0F` of the per-list lookup (`LISTS` has 15
  lists), strings of both kept. 1,506 host tests; fw gate: resident
  81,075 B (+32 B of strings crossed a 256 B stack step: usable 19,456 B),
  stack margin 4,240 B. Suite 16/16 on `9a3bce6a` (idle Home 822 line
  loads/s, stack free 6,800 B).
* Build warning (existing): a 17.8 KB anonymous rodata section is larger
  than a rodata group and stays resident-mapped; open: find its owner.

### 2026-11-25 — GAME-6 merged; Bricks on the player (lead)

* Merged GAME-6 (`ebbf8ce3`) with Salvo: firmware features `games`,
  `salvo`, `bricks`; the in-place save decode (STACK) gained Bricks;
  suite mask: values column (four rows) and the list's scroll bar. Gate:
  1,429 host tests, fw build/size/resident, stack margin 4,496 B.
* Fixed two problems the GAME-6 worker found (`2ca82de5`): Salvo's pinned
  checksum hashed whole saves including their CRC, so it pinned nothing
  (`regress_salvo_pin_depends_on_the_game`, re-pinned `0xce76c783`);
  `tests/settings/effects.toml` was not TOML (three oracles with
  unescaped quotes; `regress_settings_manifest_is_valid_toml`).
* On the player (`2ca82de5`): Games lists five games (scroll bar);
  Bricks opens (after "Loading…" ≈ 2.5 s with music: the games file
  pieces), level 1, launch, a brick breaks (10 points), a lost ball
  returns to "OK to launch". While Bricks plays: ≈ 14 UI loops/s, busy ≈
  46 %, ≈ 2,700 line loads/s. Underruns came only from the games file
  pieces (`serve: games 51 ms … pl=217 r=2 w=2 bp=2`, `job:games 49 ms`):
  5 over two game opens and quits — the `games-io` cluster work.
  Hands-on play (power-ups, levels, three balls) is the owner's check.

### 2026-11-25 — Salvo on the player; games pieces are paging-bound (lead)

* Merged STACK (`e2a55208`: key derivation finish split by kind, games
  save decode in place, one-time codes read and hashed in separate frames;
  margin 4,560 B with Salvo on) and the settings lookups by list:
  margin 4,520 B, then 4,496 B with the new log counters.
* **Salvo on the player** (`1515866f`): in the Games list (4 rows),
  plays against the computer with music: terrain, wind, shells, the
  between-rounds shop; 0 underruns over a round, stack free 7,056 B;
  pause menu; Quit saves (`Salvo — Continue`). While Salvo animates the
  UI is busy ≈ 48 % at ≈ 2,700 line loads/s (open: a Salvo hot cluster).
  Idle Home: 18 ms per UI loop, 570 line loads/s.
* **Games pieces during music are paging-bound**: the `serve:` line now
  counts pager loads and card I/O (`e8edd7e8`): `serve: games 40 ms while
  playing pl=170 r=2 w=2 bp=2`, `53 ms pl=223 r=3 w=0`, `37 ms pl=156 r=1
  w=2` — 156–223 code lines paged in for 1–3 card blocks. Six Salvo
  continue/quit cycles with music: 3 underruns (`ub=serve:games`,
  `job:games` 41 ms). A worker builds a call-graph-derived `games-io`
  cluster (tooling `xtask hot --from`).
### 2026-11-25 — AU-PERF: the games pieces' code in one cluster; `xtask hot` (worker, no hardware)

The lead's measurement (games switched while an MP3 plays): `serve: games
53 ms while playing pl=223 r=3 w=0 bp=0` (and 40 ms pl=170, 37 ms pl=156):
1–3 card blocks, but 156–223 code line loads per piece. A fault capture
of a game switch is dominated by playback and UI faults, so
`--update-hot` would pick the wrong sections.

* **Tool** (`xtask/src/hot.rs`; 07 §5.6): `cargo xtask hot [--name NAME]
  [--from PATTERN]… [--stop PATTERN]… [--lines N] [--fits-with A,B]
  [--top N] [--write]`: the paged code reachable from the roots in the
  call graph of `xtask stack` (direct branches and `stack.toml`'s global
  indirect targets; the panic paths and `--stop` not entered): functions,
  paged and resident bytes, by crate and by owning cluster, the largest
  functions; then a cluster of the reachable paged functions closest to
  the roots first, within N lines, leaving functions other clusters own
  to them. `--write` puts it in `hot.toml` with its `from`, `stop` and
  `budget_lines` (the order already there is kept, so a hand-tuned order
  survives). `xtask stack` checks every cluster with `from`: the
  reachable paged code outside it and its `fits_with` costs a line per
  started KiB of each function, and the cluster's lines plus those must
  stay within `budget_lines` (`--check` fails otherwise). Pagelink:
  `starts_over = "NAME"` starts a cluster on NAME's physical line (the
  code window before it left empty).
* **Measured** (static, firmware ELF): before, the games step
  (`Server::games_step`, everything of `games::piece` and `CardRun`
  inlined into its 4,386 B poll) reached **51,526 B of paged code (51
  lines) in 86 functions** (+3,798 B resident): 38,002 B in no cluster
  (63 functions), 8,042 B in `dir-walk`, 5,224 B in `play-fill`. Large
  parts are never run by a save or a read: a new file's creation
  (`create_entry` 7,524 B), `remove` (3,438 B), `create_dir`,
  `set_len`, the allocation, `write_data`'s growth (`extend`). Not the
  cause: generic duplicates (one `Volume<Card>` instantiation), format
  paths (none reached besides the panic stubs), exFAT and FAT (both
  branches are in the same few functions: `update_entry`, `set_dirty_flag`;
  exFAT's `exfat_fix_checksum` 1,072 B). What a save of an existing file,
  a read and a score load run: **25,974 B (26 lines)**, of it 7,058 B in
  `dir-walk` and 5,224 B in `play-fill`, and **13.5 KB in no cluster,
  scattered over 27 functions** (the games step 3.3 KB, `update_entry`
  1.6 KB, `write_data` 1.3 KB, `exfat_fix_checksum` 1.1 KB, `open` 1 KB,
  the card write path with its busy-poll timer, the path parsing). With
  the scattered lines evicting each other (and the playback's) across
  every card wait, that is the 156–223 loads.
* **Changes** (no behaviour change):
  1. `CardRun::step` (orpheus-apps): the stages a save, a read and a
     score load never reach (a new file's space search, creation and
     folders, a removal) are `CardRun::rare_step`, a future polled through
     `poll_apart` (a function of its own; the future pinned in `step`'s
     state with `pin!`: an `async fn apart(fut)` kept it twice, +464 B of
     .bss and −512 B of stack margin, measured and undone). The games
     step's poll 4,386 → 3,206 B. Its tests unchanged and green
     (`regress_games_card_ops_in_pieces`,
     `regress_games_save_first_piece_owner_card`).
  2. `hot.toml` cluster **`games-io`**: 28 functions, 13,550 B, **14
     lines**, generated by `cargo xtask hot --name games-io --write` with
     `stop` = `rare_step`, `create_entry`, `set_len`, `extend` (what the
     operations above never run), `budget_lines = 14`,
     `fits_with = ["play-fill", "dir-walk"]` (the card reads and the path
     lookups run in the same piece). Placement: `play-fill` (physical
     lines 2–7), `games-io` (8–21), `dir-walk` (22–0, moved before
     `flac-frame`; `fits_with = ["play-fill"]` added), then `flac-frame`
     with `starts_over = "mp3-frame"` (back on lines 8–16; 7,168 B of the
     code window left empty: 118 code pages, slot +8 KiB). No two of
     play-fill, games-io and dir-walk share a line (pagelink's check).
     Order within `games-io` by hand: what runs again after every card
     wait (Cache::write_slot, the card and SD writes, `wait_ready` and its
     `Timer`) on `mp3-frame`'s lines, which the audio task's resumption
     into a chore never touches; the helpers `open` runs once on lines
     19–21, which share with the audio task's poll.
  * **Not met as asked**: "no physical line shared with `mp3-frame`,
    `play-block` and `play-fill`". The three take all 32 lines (11 + 15 +
    6), so every other line shares with one of them. `games-io` shares
    with `mp3-frame` (8–18) and `play-block` (19–21); `dir-walk` with
    `play-block`. They do not run during a piece (the piece runs inside
    the audio task's loop), so the cost is the playback reloading at
    most those lines after a piece. And ~8 lines was not reachable
    without behaviour changes: what a save runs is ≈ 13.5 KB besides the
    walk and the card reads.
* **Expected on the player** (lead to measure): per games piece the
  lines it touches in `games-io` (≤ 14, a save's first step nearly all,
  a flush step ≈ 6, a read ≈ 8), in `dir-walk` (≈ 7 of its 11, the path
  lookup), `play-fill` mostly warm, plus the unclustered dispatch
  (`Apart<chores>` 3 KB, `Server::serve` 1.2 KB, `Server::step` 3.6 KB)
  and 1–3 per card wait where the audio task's resumption chain (its
  task poll on line 19, `run_player`/`play_file` on 27–1, `chores` on
  0–3) meets `games-io`'s last lines and `dir-walk`'s last six: **≈
  15–30 loads per piece (3–6 ms) instead of 156–223**. A piece under
  30 ms is not logged as `serve:` at all: the check is that `serve:
  games` lines are gone, and `chore: … job:games` / `ub=` stay quiet.
* **Other chores while playing** (static reach of each piece; the second
  number without file creation, growth and removal, which these do not
  run): `job:index` (the `find_index` session) 50 / 31 lines, of which
  10.5 KB in `dir-walk` and ≈ 10 KB now in `games-io` (`open`, the entry
  and write path); `job:rows` focus 48 / 29 lines; `job:rows` fill 54 / 35
  lines (14 KB in no cluster); `job:win` 16 lines; `job:count`
  (`read_dir`, 3,962 B) 11 lines; the listing step as a whole 65 / 46
  lines. The same problem (working sets of 29–35 lines, much of it in no
  cluster); not fixed here. `job:index`'s sessions are synchronous
  (`spin_in`), so its stretch does not cross the audio task's resumption;
  its code (dir-walk + games-io + play-fill) is now on disjoint lines.
  Next: `cargo xtask hot` clusters for `job:rows` (its 14 KB of index
  reading), and the games path's walk avoided (reopen the save from its
  entry, as listing pieces reopen the index file) to free `dir-walk`'s
  lines from the games piece.
* **Tests**: `xtask` `hot::tests` (`distances_stop_and_roots`,
  `selection_closest_first_within_the_budget`,
  `selection_keeps_its_own_and_skips_shared_names`,
  `working_set_counts_the_cluster_and_each_function_outside`,
  `regeneration_keeps_the_order_in_hot_toml`,
  `hot_toml_games_io_is_checked_and_placed`,
  `regress_games_piece_working_set_over_its_line_budget`: the measured
  156 scattered functions fail an 8-line budget, the clustered set passes
  it), `pagelink::tests::starts_over_pads_to_the_named_clusters_line`.
  On the device: the `serve:` line's `pl=` (lead).
* **Numbers**: resident 81,043 B, .bss 125,272 B (both unchanged); stack
  15,216 B nested worst case of 19,712 B usable, **margin 4,496 B**
  (unchanged; budget 4,096); code pages 117 → 118 (the 7 KiB left
  empty), slot 1,208,320 → 1,216,512 B.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings` (and `orpheus-apps` with `games`, with `games,salvo`), `cargo
  test --workspace` (1,409 passed, 11 ignored), `xtask test pagelink`;
  fw `fmt --check`, `clippy --workspace -D warnings`, `cargo xtask
  build`, `check-resident`, `size --check`, `stack --check` (with the
  `games-io` line budget: 14 of 14): pass. Not run: every `xtask dev`
  step (the lead owns the player).

### 2026-11-25 — UI-PERF: settings lookups go to their own list (lead)

* Fault profile of two game switches during music (`dev prof --faults 1
  --during "keys …"`, 17.6 s, 3,072 line loads/s): the two hottest
  sections were **settings tables in rodata** (`.Lanon…905` = the games'
  settings, `.Lanon…922` = a group's), 27 % of all line loads; idle Home
  showed them too. Every `def`/`index` scanned all ~150 items (3.6 KB in
  several rodata lines), and the UI looks settings up many times a frame.
* Fix: keys are grouped by their high byte (groups `0x01–0x0C`, apps
  `0x0D`, games `0x0E`); `def` and `index` search only that list
  (`LISTS`, a static; `FIRST_STORED` offsets computed at build time);
  `all()` iterates `LISTS`, so the games' table (a `const` array) is no
  longer copied twice into rodata. `#[inline(never)]`: inlined, the UI
  task's frame grew 40 B. Tests: `keys_are_grouped_by_their_high_byte`,
  `regress_settings_lookup_loops_match_the_iterator_form` (no feature,
  `games`, `games,salvo`).
* On the player (before Salvo's merge): idle Home 27 ms per UI loop (was
  47), 704 line loads/s; five game switches during music **0 underruns**
  (was 1; 12 two builds ago). The games pieces still take 32–36 ms
  (`serve: games … while playing`): open.
* Tooling: `dev keys` takes every word of its script (`be2fc7af`, with
  `regress_keys_takes_every_word_of_the_script`).

### 2026-11-25 — STACK: room for Salvo; Salvo on in the firmware (worker, no hardware)

Salvo did not fit the static stack budget (GAME-4/5 entry below: margin
3,604 B < 4,096 B with it). Three cuts of the UI task's deepest paths,
then `salvo` added to `orpheus-apps`' features in
`fw/firmware/Cargo.toml` and the `games` journey's fourth-row mask
removed (`tests/suite/journeys.toml`: the values mask now covers all four
rows, 33..110).

* **Numbers** (`cargo xtask build`, `size`, `stack`; Salvo on):

  | | before (Salvo on) | after |
  |---|---|---|
  | worst thread path | 6,796 B: `derive` → `derive_finish` (4,008-B frame) → `seal` → `Pool::fill` → SHA-256 | 5,840 B: Authenticator list `reload` → `auth_code_now` (400) → HMAC-SHA-512 (`mac` 1,232 + `compress512` 1,448) |
  | nested worst case | 16,108 of 19,712 B usable | 15,152 of 19,712 B |
  | margin (budget 4,096) | **3,604 B** | **4,560 B** (+956; 464 B slack, 1.8 steps of 256 B) |
  | resident | 81,043 B | 81,043 B (budget 81,920) |
  | `.bss` | 125,232 B | 125,232 B (budget 126,976) |

  The rt context (8,144 B, the card path under `media::Server::step`) is
  unchanged; it is now the larger half of the nested worst case.
* **Cuts** (no RAM moved, no behaviour change):
  1. `orpheus_otp::service::Authenticator::finish` split into
     `finish_unlock`, `finish_create`, `finish_change` (`finish` calls
     them; another kind's job is refused with `Event::Failed`, nothing
     written). The firmware's `Fw::derive_finish` dispatches on
     `Job::work` to three `#[inline(never)]` frames, each polling one
     kind's future: one future for all three kept the unlock's,
     set-up's and PIN change's states side by side (4,008 B; now
     `finish_change` 2,728 B, `finish_create` and `finish_unlock` less).
     The derivation itself (`Job::step`, `Vault::finish_*`) is untouched:
     `per_kind_finish_equals_finish` runs set-up, import, PIN change and
     unlock both ways and compares the sector bytes and the code (equal,
     bit for bit); the KDF and vault test vectors pass unchanged.
     `derive` 6,652 → 5,372 B from the UI task.
  2. `Session::decode_small` decodes Blocks/2048/Solitaire into the
     session in place (it returned a 680-B `Session` into
     `decode_inner`'s 1,480-B frame; now 144 + 1,392 B), and
     `GamesIo::done`'s score-table decoding is a function of its own
     (`games::loaded`; the ≈ 490-B table stayed in `done`'s 1,008-B frame
     under a save file's decoding; now 24 B). The games path (settle →
     … → `games_poll` → decode) 6,432 → ≈ 4,450 B from the UI task (`games_poll` 4,496 → 2,512 B).
  3. `Authenticator::code_of` (the code of an entry already read; `code`
     uses it). The firmware's `auth_code` and `auth_type` read the entry
     in one `#[inline(never)]` frame (`auth_read_entry`) and hash in
     another (`auth_code_now`): `block_on(code)`'s 640-B future no longer
     sits under the HMAC. `AuthScreen::reload` 752 → 440 B (the code was
     inlined into it). That path 6,248 → 5,696 B from the UI task.
  Next in line from the UI task: `derive` 5,372, `new_ui` 5,064,
  `auth_run` 4,888 B. The next ≈ 324 B would need the HMAC-SHA-512 frame
  (`hmac` crate's `finalize` copies, 1,232 B) and the derivation's
  `finish_change` cut together.
* **Salvo on the UI task** (`xtask stack --root`): `Salvo::step` (the
  computers' search, one simulated shot, is inlined into it) 752 B with
  the panic tail, under `event_of<GameScreen>` 984 B, ≈ 2,530 B on the
  thread; `SalvoView::draw` 1,192 B, ≈ 3,330 B on the thread through
  `Ui::draw` (the worst draw is Now Playing's details, 2,512 B). Neither
  is near the worst path. Host cost (release, `measure_cost`, re-run):
  a decision 0.41–2.05 ms (worst 2.60), worst logic step 0.437 ms (one
  simulated shot, the Moon), 25 ns per physics step, 1.0 µs per flight
  logic step: unchanged from GAME-4/5; player estimate ≤ 20–25 ms per
  logic step with a shot (one per frame).
* **Tests**: `orpheus-otp` `per_kind_finish_equals_finish`,
  `per_kind_finish_refuses_other_kinds`; `orpheus-games`
  `decode_into_replaces_the_session` (each game's file over each other
  game, none, and Salvo with the feature: replaced; truncated or another
  game's file: no game). No bug fixed, so no `regress_` test.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings` (and `orpheus-apps` without features, with `games`, with
  `salvo`; `orpheus-games` with and without `salvo`), `cargo test
  --workspace` (1,400 passed, 11 ignored; goldens unchanged); fw `fmt --check`, `clippy --workspace -D
  warnings`, `cargo xtask build`, `size --check`, `check-resident`,
  `stack --check`: pass. Not run: every `xtask dev` step (the lead owns
  the player).
* **Lead, on the player**: Salvo is in this build. Check: the Games list
  shows four rows (`dev suite` journey `games`, now unmasked); play Salvo
  against computers while music plays (underruns, UI loop time, the
  computers' thinking frames); Continue after a reboot; the
  Authenticator's unlock, set-up, PIN change and a code (the per-kind
  finishes; same vault bytes on the host); stack free in `status` after
  those (≥ `stack_free_min_bytes`).
### 2026-11-25 — GAME-6: Bricks, the brick breaker (host, simulator, firmware build; worker, no hardware)

Design, rules, keys, settings and numbers: 05 §7.13 "As built (GAME-6
Bricks)".

* **Engine** `orpheus_games::bricks` (feature `bricks`; `Bricks` 256 B, so
  the session and the hub do not grow): integer physics (1/256 px, a sine
  table in 1/4096) — the thumbv7em build of it has **no floating-point
  instruction** (0 `v…` in the 18 Bricks functions, 0 `vfma` in the image).
  Swept collision as sub-steps of at most a pixel per axis, x then y; a
  move that would overlap a wall, brick or the paddle turns the ball
  instead. Paddle angle by contact (12°–60° from the vertical), the paddle
  stops at a ball beside it; speed-ups by level and over a rally; a stall
  nudge against endless loops between unbreakable bricks. Eight hand-made
  levels of our own and generated ones (five symmetric patterns, density
  and hardness by level, unbreakable from level 4, pockets opened by a
  reachability search). Power-ups: wider, slower, split (3 balls), catch,
  life. Lives, score (10 per hit a brick took, 100 per level), save
  `bricks.sav` 255 B, score table (variant from six settings).
* **App and simulator**: `Game::Bricks` (Games list fifth), screen
  `Bricks` (`screens/bricks.rs`, partial redraw: changed brick rows and the
  rectangles each ball, capsule and the paddle left and took, each
  repainted in order clipped to it; status line on change; banners on a
  phase change). A new game waits with the ball on the paddle ("OK to
  launch"); a game continued from the list in mid-rally opens paused.
  Settings `0x0EA1–0x0EA7` packed into `0x0EB0` (`STORED` 124 → 125 with
  Salvo); Steering applies at once, the rest from the next game. Simulator
  hooks `set:bricksball|brickscap|brickslevel|bricksclear|brickslives|
  bricksspeed|brickspaddle`, `goto:Bricks`. New strings appended (16).
* **Tests**: `crates/orpheus-games/tests/bricks.rs` (17): 120 random games
  (random settings, keys, steps; half with an autopilot under the ball:
  levels cleared, power-ups caught, games lost) with, after every input
  and step, `Bricks::check` (no ball in a wall, brick or the paddle, held
  balls on the paddle, counts, phase), bricks only losing hits within a
  level, the score recomputed from the cells that emptied and the levels
  cleared, saves read back equal, the run repeated equal; **no
  tunnelling** at 5 px/step: a full row of bricks never passed (3,024
  shots: 7 angles × 2 sides × 54 places × 4 sub-pixel offsets), the
  paddle never passed (144 drops), and over 400 random max-speed balls in
  generated level-10 fields no step that turns nothing has a brick on its
  straight path (the brick less the pixel the staircase may stray);
  mutation check: with one sub-step per step both brick tests fail; walls
  and unbreakable bricks at max speed; paddle angles; serve and keys;
  lives and game over; levels cleared and the generated set; hits and
  points; speed-up and cap; every power-up; save round trip;
  **deterministic replay pinned** (checksum 0x8a1fd172 over 120 saves of a
  12,000-step autopilot game, each save without its own CRC — see open
  item 1). Engine unit tests: every level of each set reachable and
  symmetric; pockets opened. Fuzz target `bricks_keys` (body in
  `fuzz/src/lib.rs`, autopilot bit) with 8 seeds in
  `tests/fuzz-corpus/bricks_keys/`, replayed by `tests/fuzz_corpus.rs`;
  `cargo fuzz` 150 s: 369 runs, no finding. `orpheus-apps`:
  `bricks_packed_settings`, `settings_table`. Simulator
  (`sim/tests/games.rs`): differential tests of all seven settings
  (`bricks_settings::*`; Bricks added as a context every other game
  setting must not change), `bricks_saved_and_continued` (pause → file
  equal to the game, quit, restart → the same game, a lost game recorded,
  the save removed), `bricks_partial_redraw_equals_full` (4 seeds × 120
  random keys, capsules, extra balls, max speed). VER-13 manifest: 8 new
  entries.
* **Goldens**: 45 new `games_bricks_*` (serve, moved, play, life lost, two
  of capsules, wider, split, catch, worn bricks, levels 5 and 8, a
  generated level, cleared, next level, over, paused, pause menu,
  settings and their end, help and its end; Dark and Light; worn bricks
  and capsule signs in Amber) and 2 `games_list_bricks_*`; the four Games
  list goldens changed (a scrollbar: six games). Looked at all; fixed from
  looking: settings labels cut off ("Ball spe…", "Paddle…" → Ball, Paddle,
  Steering).
* **Suite**: the `games` journey's list shot gets a mask over the
  scrollbar (the simulator lists six games, the firmware five);
  `expected/games/list.png` re-made through the test (`ORPHEUS_BLESS=1`,
  not `xtask dev`).
* **Measured**: host (release) 80 ns per logic step; game loop per 33-ms
  frame (simulator, all services) in play 3.6 µs and 30.6 dirty rows.
  Player estimate (× 40–60): ≈ 5 µs a step, ≈ 0.2 ms a frame before the
  flush; the generated level's reachability search runs once per level.
* **Firmware** (`cargo xtask build`, `size --check`, `check-resident`,
  `stack --check`, all pass): **Bricks is in** (`orpheus-apps` feature
  `bricks` in `fw/firmware/Cargo.toml`). Without it (the shared changes:
  strings, settings tables): resident 80,779 → 80,795 B, `.bss` and stack
  unchanged. With it: resident 80,923 B (+128), `.bss` 124,872 B (+8, the
  settings word), slot 1,175,552 → 1,191,936 B (two code pages), stack
  16,108 B nested worst case of 20,224 usable, **margin 4,116 B
  unchanged** (the thread context stays 6,796 B; Bricks' deepest frames:
  `load_level` 992 B incl. the level generator, not on the deepest path).
* **Gate**: host `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, and per crate `orpheus-games` (none, bricks,
  salvo, both) and `orpheus-apps` (none, games, bricks, salvo, both) with
  `--all-targets -D warnings`; `cargo test --workspace` (1,424 passed, 11
  ignored); fw `cargo fmt --all --check`, `cargo clippy --workspace -D
  warnings`, `cargo xtask build`, `size --check`, `check-resident`,
  `stack --check`: pass. **Not run**: `xtask test all` beyond these (fuzz
  smoke, codecs), `brand --check`, `site --check`, `xtask wasm` (the
  browser build has Bricks through the simulator), every `xtask dev` step.
* **Open**: (1) Salvo's `deterministic_and_pinned` (and the first draft
  of this one) checksums `crc32` of whole save files: a CRC-32 over data
  followed by its own CRC is always the residue 0x2144df1c, so XOR-ing
  those pins nothing (the Salvo test passes for any game); it should hash
  `buf[..n - 4]` and be re-pinned (left to the Salvo worker: not changed
  here). (2) `tests/settings/effects.toml` is not valid TOML at the Salvo
  entries 0x0E62/0x0E63 (unescaped quotes in `oracle`, line 274);
  `settings_table` only greps it; the VER-13 gate that parses it will
  fail there. (3) Expect merge conflicts with the Salvo-in-firmware work
  in `fw/firmware/Cargo.toml` (one feature) and `journeys.toml` (one mask
  rectangle).
* **Lead, on the player** (user check): Home › Games › Bricks (fifth row;
  fourth if Salvo stays out): launch, move the paddle (Prev/Next, hold to
  speed up), break bricks, catch a capsule of each kind (bar = wider, S,
  three dots, C, +), lose a life, clear level 1 (level 2 follows after
  the banner), Back → pause → Quit → "Continue" → the same game paused;
  Continue after a reboot. Watch the frame rate with three balls (the
  estimate is ≈ 0.2 ms of drawing a frame plus the flush of ≈ 30 rows) and
  music playing (underruns while switching to and from Bricks, as for the
  other games).
| BOOK-1 | **done (host + simulator)**; not in the firmware (feature `orpheus-apps/books` off there: stack/RAM, see session entry) | `orpheus-books` (encodings TXT/Markdown/FB2 → normalised stream cached in `/.orpheus/cache/books/`, layout and page index, positions, bookmarks, contents, library search; all card work in bounded steps), Books app (library, reader, reader menu, contents, bookmarks, Go to; 7 reading settings packed in 0x0F80), reading faces 16/20 px (`orpheus-ui/book-fonts`), Home scrolls to a fourth row; 92 goldens; fuzz targets `books_convert`, `books_layout` (session entry 2026-11-25 "BOOK-1") |

### 2026-11-25 — BOOK-1: Books app and reader core (worker, no hardware)

Host and simulator; **not in the firmware** (feature `orpheus-apps/books`
off there, see "Firmware" below). Design and as-built details: 05 §7.15
"As built (BOOK-1)".

* **Core crate `orpheus-books`** (`no_std`, no alloc, no `core::fmt`,
  ≈ 7.2 k lines with tests): encoding detection and a streaming decoder
  (UTF-8 ± BOM, UTF-16 LE/BE with and without BOM, XML declarations,
  Windows-1251, KOI8-R, Latin-1/1252; malformed input → U+FFFD);
  TXT (hard-wrapped or line-per-paragraph, chapter headings, scene
  breaks, Gutenberg title/author), Markdown (reduced set), FB2 (XML
  tokenizer, sections → H1–H3, titles, epigraphs, verse, notes, emphasis)
  → one **normalised stream** cached in `/.orpheus/cache/books/<id>.txt`;
  `layout` (Liner/Pager/Paginator, `Mark` = offset + 1 state byte, the
  page function the reader draws with); `files` (page index `.idx`,
  contents `.toc`, position and 12 bookmarks `/.orpheus/books/<id>.pos`,
  library `/.orpheus/books/library.bin`); `shelf::Shelf` (the app model
  and all card work as `step()`, one bounded piece per call).
* **Measured** (FAT32 and exFAT images, `tests/shelf.rs` with a counting
  block device): ≤ 16 sectors read and ≤ 12 written in any step (12 only
  when a folder is made and its cluster zeroed); all other writes ≤ 3.
  Host speeds (debug-opt-1): converting and laying out the sample books
  is a few hundred steps; nothing measured on the player.
* **RAM** (host `size_of`, `tests/ram.rs`; all fixed; a ratchet at
  9,472 B): the whole `Shelf` **9,360 B** — `Book` 3,828 B (text window
  2,048, contents window 8 × 48, page-index cache 64 × 8, position and 12
  bookmarks 444, path 258), `Library` 1,158 B (sort order 200, 8 rows),
  and one card job at a time in a union (the search: its 840-byte
  `DirEntry`, 8 folder cursors, 200 sort keys, the meta reader 524 B and a
  384-byte record; or the converter 356 B with 512 B in and 1 KiB out; or
  the paginator 96 B with 512 B in and 128 held index entries). The
  reader's draw path adds a `Liner` (64 B) and `Pager` (28 B); the
  screens' variants are no larger than the existing list screens.
* **UI** (`orpheus-apps`, feature `books`; `orpheus-ui` feature
  `book-fonts`): Books on Home (13 apps scroll to a fourth row; off Home
  unless the system offers it); library (title, progress, searching,
  empty; menu: Search again, Find books in); reader (pages, lines,
  chapters, position bar, palettes, auto page turn, quick menu); reader
  menu; contents; bookmarks; Go to. Reading faces DejaVu Sans Condensed 16
  and 20 px (14,468 + 20,244 B rodata, `fontgen/regen.sh`), Extra large =
  UI font ×2, drawn bold/slant/scale in `Gfx::char_styled` (only with
  `book-fonts`: the firmware's `orpheus-ui` is unchanged). New icon BOOKS
  (`icongen/icons.py`; the other icons regenerate byte-identical).
* **Settings** 0x0F01–0x0F07 (Font size, Margins, Spacing, Justify,
  Palette, Auto page turn, Find books in) packed in one stored word
  0x0F80 (stored settings +1 with the feature); `tests/settings/effects.toml`
  entries and differential tests (`sim/tests/books.rs::settings_effects`).
* **Simulator**: `sim/src/books.rs` (the Shelf over the simulated card,
  `set:books=on`, `set:booksteps=N`), three public-domain books added to
  `/Books` (Carroll in Markdown, Melville in UTF-16 with a Gutenberg header,
  Pushkin in Windows-1251 FB2); the text-viewer goldens' book rows moved
  (`{book}` 2→4, 3→6: the same screens).
* **Tests**: 41 unit tests in `orpheus-books` (encodings incl. malformed,
  parsers, writer, layout: no word lost or duplicated, piece sizes, stable
  index, re-layout keeps the position, line up, bounded pages; files round
  trips incl. bookmarks and position, damage), `tests/shelf.rs` (whole
  pipeline on FAT32 and exFAT, every page through the reader path, jumps,
  bookmarks and position across a reopen, re-layout, scope, only
  `/.orpheus` written (write prefix), fsck clean; a changed book
  reconverted), `tests/ram.rs`; `orpheus-apps` (settings table, faces and
  params, Home's fourth row); `sim/tests/books.rs` (15: library, every page
  by Next/Prev, restart, re-layout, layout in steps with the reader
  answering, books never written, auto turn, Find books in, 7 differential
  tests); fuzz targets `books_convert` and `books_layout`
  (`crates/orpheus-books/fuzz`, 18 seeds in `tests/fuzz-corpus/books_*`,
  replayed by `tests/fuzz_corpus.rs`); **92 goldens** (`books_*`: dark and
  light, two amber), all looked at.
* **Bugs found and fixed during the package** (each with a `regress_`
  test that fails without its fix): the layout job re-read a character
  cut between pieces (pages after it a few bytes off;
  `regress_layout_pieces_resume_after_a_cut_character`); after the page
  view refused a line, the liner still gave it a scene break's line, which
  the view took as its page end (Next skipped a line; found by the
  `books_layout` seed `fb2`; `regress_no_line_after_a_refused_one`); an
  invalid UTF-8 sequence cut between pieces overflowed the piece index
  (`regress_invalid_sequence_cut_between_pieces`, seed `garbage`).
* **Firmware: not wired.** Not attempted because it cannot pass
  `stack --check` today: the UI task's margin is ≈ 4.1 KB against the
  4,096 B budget and another worker is making room for Salvo; Books adds
  static RAM (the Shelf, 9,360 B) and the reader's draw path (layout +
  styled glyphs) to the UI task. What it needs: (1) the Shelf split like
  `GamesIo`/`CardRun` — the screens' model (text window 2 KiB, rows,
  contents rows, state ≈ 3.5 KB) on the UI side, the card work (the
  scan's 840-byte `DirEntry`, the converter, the paginator, the scratch
  buffers) in the card task — or one static behind a BASEPRI
  critical-section lock; (2) the step futures' large locals moved into
  the struct (index chunk 512 B, the layout's last write 1 KiB, the
  open's state/record/`DirEntry` ≈ 1.7 KB, library header 272 B) so the
  card task's stack does not grow; (3) `Shelf::step` as a paced chore
  (`orpheus_audio::chore`: conversion and layout only with headroom, the
  window read and jumps first; folder creation zeroes a cluster — 256
  sectors on the owner's 128 KiB-cluster card — so it needs its own chore
  or no music); (4) rodata ≈ 35 KB of faces plus the code, to be checked
  against `rodata_pages`; (5) `App::Books.on_player()` true and the
  suite's Home masks updated.
* **For BOOK-2 (EPUB)**: a zip/inflate reader (the 32 KiB window in the
  decoder arena when no track is loaded, 05 §7.15), OPF spine and NCX/nav
  contents, an XHTML parser into the same `norm::Events` (the FB2 tokenizer
  can be split out and shared), chapters concatenated in spine order into
  the one normalised stream (contents from the nav, not the headings),
  `Format::Epub`, OPF metadata in `MetaReader`; everything after the
  stream (layout, index, positions, screens) is unchanged.
* **For BOOK-3 (read aloud)**: positions are stream offsets (`Mark`): feed
  `orpheus-tts` from the view's first line (strip control bytes,
  paragraph bytes as sentence ends); the TTS word offsets map back to
  stream offsets, the reader knows each character's x (`draw_line`) for the
  sentence highlight; follow pages with `PageInfo::next` when speech passes
  it; resume with `Req::Seen` of the spoken sentence; the reader menu's
  Read aloud row is there (greyed). Needs the speed setting (0.25–10×),
  voice, sleep timer and screen-off reading.
* Open: on-device run (the firmware integration first); hyphenation (Extra
  large breaks long words mid-word); Markdown setext headings and tables;
  translations of the Books strings; an empty-library golden (the
  simulator's card always has /Books; a host test covers the state).
* Gate: fmt; clippy `-D warnings` on the workspace and on `orpheus-apps`
  (none, games, salvo, books, games+books, salvo+books) and `orpheus-ui`
  (none, book-fonts, menu-key, both); `cargo test --workspace`: all pass
  (three xtask tests assumed the simulator's Home is the player's: with
  Books compiled into the workspace's `orpheus-apps` they counted 13 apps;
  `soak`'s Home positions, the suite's Home frames and the test-look mask
  now count only `App::on_player()` apps — otherwise `dev soak`/`dev
  suite` would have navigated the player's Home one cell off; rerun
  113/113), the simulator's Books, golden and apps tests re-run after the
  last changes. **Firmware** (Books not wired; it changes only by the 42
  new English strings, and `settings::all` and Home compile to the old
  code without `books`): `xtask build --release` ok, `size --check` within
  budgets (resident 80,811 B, .bss 124,864 B, 113 code + 20 rodata
  pages), `check-resident` ok, `stack --check` 16,108 B of 20,224 B,
  margin 4,116 B (budget 4,096 B). Not run: `dev` (no hardware in this
  package).

### 2026-11-25 — UI-PERF: idle Home thrashed the code cache; games during music on the player (lead)

* **Regression found on the player** after merging the in-place saves
  (`f3f39b9d`, no firmware file changed): idle Home took **1.7 s per UI
  loop**, 2,939 line loads/s, each key 3.3 s, and game switches during
  music ended in watchdog resets. A/B through `dev update`: `6a5a7f8e`
  idle Home ≈ 730 loads/s, 45 ms per loop, keys 0.2 s each.
* Fault profile (`dev prof --faults 15`, idle Home): 86 % of faults on
  `orpheus_apps::settings::index` and the closure `Iterator::position`
  called out of line for every item — the two shared a code-cache line in
  that layout (it moved by 32 B of resident code), ~260 line loads per
  lookup of ~130 items.
* **Fix** (`ad1bbd16`): `settings::def` and `settings::index` as plain
  loops over the tables (one function each: 158 B and 218 B, no closure).
  On the player: idle Home 760 loads/s, 47 ms per loop, 5 keys in 1.1 s.
  Tests: `regress_settings_lookup_loops_match_the_iterator_form`
  (orpheus-apps, with and without `games`); the suite now samples the
  pager over 5 s of idle Home and fails above 1,500 line loads/s
  (`regress_idle_home_thrash_fails_the_suite` with the measured figures):
  a layout collision on the idle path cannot be caught on the host.
* **Games during music** (five game switches with an MP3 playing, on
  `ad1bbd16`): **1 underrun** (was 12 on `6a5a7f8e`, 8 before the pieces),
  no reset; music kept playing throughout. Remaining: some games pieces
  take 33–40 ms (`serve: games 38 ms while playing`) against the worker's
  estimate of a few sector I/Os — most likely paging of the save path's
  code (no hot cluster). Open: a fault capture of a game switch during
  music (`dev prof --faults --during …`) and a `games-io` cluster or
  smaller pieces.
* Tooling note: with the player running from slot B (after `dev update`),
  `dev stage` refused ("slot image 1175552 bytes > slot A 491520 bytes"):
  nothing was written; `dev update` with the build's `.ofw` worked. Open:
  make `stage` read the full layout's slot sizes from either slot.

### 2026-11-25 — AU-PERF: games saves in place; the allocation search in steps (worker, no hardware)

Lead's measurement (owner's card, MP3 playing, games switched): every
save's first piece one 41 ms stretch, `chore: starved serve:games 43/26
ms u41`, 12 underruns a minute.

* **Found on the host** (`regress_games_save_first_piece_owner_card`'s
  image: sparse 238 GiB exFAT, 128 KiB clusters, a 476-sector bitmap, 300
  root folders, music in 95 % of the clusters from the start, then
  `/.orpheus/games` with saves; 8 cache slots, `/.orpheus` warmed, cold
  cache as after a start). A save of an existing file, broken down: walk 1
  read; truncating open 2 reads, 2 writes (dirty flag, entry flushed,
  cluster freed); **write 452 reads** (the allocation: exFAT keeps no
  next-free hint, so after mounting it is cluster 2 and the bitmap is
  searched from its start); entry 1 read; then 3 + 1 one-write steps.
  `CardRun` steps: (456 r, 2 w), (0, 1) ×3, (1, 1). The first save after
  every start pays it; later ones depend on the holes behind the hint (on
  the owner's scattered card, often again). FAT32 (8 GiB, 32 KiB clusters,
  2,048 FAT sectors) with a good FSINFO hint: (3, 3) first step; with the
  hint unknown (0xFFFFFFFF): **1,946 reads** in that step.
* **Fix (a), in place**: `orpheus_fs::Volume::rewrite(&mut File, data)`:
  the whole content replaced in the file's own clusters when they are as
  many as the data needs (a save, ≤ 512 B, always: one cluster): nothing
  freed or allocated, no data sector read (fresh), size and exFAT valid
  length set; otherwise `false`, untouched. `CardRun` `Write`/`WriteScores`
  try it first (`Stage::Open`: open read-write, rewrite, entry into the
  cache), then flush in steps as before.
  **Power cut**: the cache writes data before directory sectors, so a cut
  leaves the old save, the new one, or the new bytes under the old entry
  (old length); no cluster changes hands, so the volume never needs
  repair (fsck finds only the dirty flag, or FAT copies differing in the
  dirty bit itself). A save is framed with its length and a CRC
  (`orpheus_games::save::open`), so new bytes under an old length of
  another size read as no save; the old path's cut left an empty file (no
  save) too, and in addition freed/allocated clusters. Tested: a cut after
  every write count on FAT16, FAT32, exFAT (`tests/rewrite.rs`
  `rewrite_power_cut`: all three outcomes seen, fsck as stated).
* **Fix (b), hints**: freeing a chain (truncate, remove, rename over a
  file; each has the old entry on the card first) leaves the next-free
  hint on the chain's first cluster (FAT32: written to FSINFO), so the
  next allocation takes it without a search (a save written again after
  Remove: 0 reads). `Volume::find_free_step(sectors)`: the search in
  stretches; `CardRun`'s new `Stage::Space` runs it 8 sectors per step
  (255 steps at most, then the allocation searches the rest itself: only
  a nearly full card) before a new file is made (`Stage::Alloc`).
  No new statics; the hint is the existing `Volume::next_free`.
* **After** (same images, one step per piece): existing save (exFAT)
  (2 r, 1 w), (0, 1) ×3; FAT32 (2, 2), (0, 1), (0, 1), (0, 2); score table
  (0, 1) ×4. New file on exFAT: 57 steps of 8 bitmap reads, then (2, 1),
  (0, 1) ×4; FAT32 with an unknown hint 244 steps of ≤ 8. Remove + write
  again: (0, 0) ×2 then one or two writes per piece. No piece above 10
  sector I/Os.
* Tests: `regress_games_save_first_piece_owner_card` (`orpheus-apps`,
  games; both images; the first piece of a save ≤ 4 I/Os, a save ≤ 4
  reads in all, every piece ≤ 10, answers and contents right, `fsck -n`
  clean after; the old code fails it with 456 reads);
  `orpheus-fs/tests/rewrite.rs`: in place (same cluster, ≤ 1 read, refused
  untouched when the cluster count changes), freed clusters taken first,
  `find_free_step` bounded (FAT32 190 steps, exFAT 3, of ≤ 4 reads) with
  the allocation after it ≤ 2 reads, power cuts; all fsck-clean on FAT16,
  FAT32, exFAT. `regress_games_card_ops_in_pieces` green (Write 4 pieces
  of ≤ 1 write on exFAT, was 5 of ≤ 2).
* **Expected on the player** (lead to measure): a save of a game played
  before: the first piece ≈ 2 sector reads and the dirty flag's write
  (≈ 2–5 ms), then one sector written back per piece and the sync; no
  `starved serve:games`, no `ub=serve:games`. A game's first save after a
  start: ≈ 57 `job:games` steps of 8 bitmap reads (< 1 ms each at the
  ≈ 0.09 ms per read the 41 ms implies), spread over chores.
* Firmware: resident 80,747 → 80,779 B (+32), .bss 124,864 B unchanged,
  stack 16,108 B nested worst case of 20,224 B, margin 4,116 B (budget
  4,096) unchanged.
* Not changed: other writers that allocate after a start (index files,
  album art, recordings) still search from cluster 2 on exFAT on their
  first allocation; they could call `find_free_step` the same way.

### 2026-11-25 — games on the player (hands-on); a 100 ms games chore; plan: brick breaker (lead)

* Hands-on on the player (games build): Games on Home; Blocks plays
  (moves, rotation, hold, hard drop; score counts), Back pauses, Quit saves
  `/.orpheus/games/blocks.sav` and the list shows "Continue"; **after a
  reboot the saved game resumes**; 4096 slides and merges; Solitaire deals
  and auto-moves. Without music: 0 underruns.
* **With music playing** (a real MP3) while switching games: 8 underruns,
  `chore: starved serve:games 100/26 ms u100` → `pcm: … ub=serve:games:100ms`.
  A games card operation (a save written before another game is read) takes
  100 ms in one piece, more than any chore budget (≈ 42 ms), so it is
  forced after 2 s. Open: split the games file operations into budgeted
  pieces (open / write in sectors / close), or defer saves while playing.
* Plan (owner's request): GAME-6, a brick breaker ("breakout"-style; an
  original game, working name "Bricks") added to M14 (05 §7.13, 08, PLAN).
### 2026-11-25 — TTS-2: formant synthesiser and prosody, host side (worker, no hardware)

In `crates/orpheus-tts` (modules `klatt`, `targets`, `voice`, `resample`,
`speak`; "as built" in 04 §8). Not wired into the firmware, the simulator
or `orpheus-apps`.

* **Design**: a cascade/parallel formant synthesiser written from Klatt
  (1980)'s description at 16 kHz with 1 ms control ticks; KLGLOTT88-like
  voicing (closure band-limited by polyBLEP, F0 not quantised), aspiration
  and frication noise, F1–F6 cascade with nasal pole/zero, parallel branch
  for fricatives and bursts; phoneme targets from Peterson & Barney,
  Klatt/MITalk, Hughes & Halle, Stevens & Blumstein, Lisker & Abramson
  (cited in `targets.rs`); locus equations for transitions; stops with
  closure, burst and aspiration; intonation with declination, accents,
  fall/rise/continue/exclaim/wh-fall on the last stressed syllable, lower
  and faster asides, `pitch_depth_pct`; three voices; polyphase resampler.
  `Synth` takes tokens, `Speaker` takes text; events (sentence, word,
  end) at their output frame; stop now (3 ms fade) or at the next word.
  `cargo run -p orpheus-tts --example say -- "text"` writes a WAV.
* **Measured**: RAM `Synth` 1,936 B, `Speaker` 3,352 B (≤ 8 KiB). Host:
  31 ns per 16 kHz sample; 20 ns per 48 kHz stereo frame with the
  resampler; front end 86 µs per second of speech at 10×. Player estimate
  (disassembly, ±30 %): ≈ 10.8 M cycles/s at 48 kHz = 7.5 % of 144 MHz,
  **22 % of Base 48 MHz (budget 20 %)**, 27 % at 44.1 kHz, 12.5 % with
  16 kHz output; front end at 10× ≈ 1 % of 144 MHz. Code (thumbv7em, opt
  "s", no LTO): TTS-2 adds ≈ 16 KB code + 5.3 KB tables, **≈ 70 KB without
  the lexicon, over the 64 KiB plan budget**. Formants by LPC within 12 %
  (F1) and 3 % (F2, F3) of target; peak −3.1 dBFS over all voices and
  speeds; mean F0 within 8 % from 0.25× to 10×.
* **Intelligibility**: whisper.cpp 1.9.4 was installed (Unsloth's
  `whisper-server`), without a model: **I downloaded `ggml-base.en.bin`
  (148 MB, SHA-256 a03779c8…6d002) into the session's scratch area, not
  the repository**. Neutral WER 13.9 % at 1×, 30.5 % at 2×, 93 % at 4×
  (6× 98 %, 10× 100 %); Low 13.4 / 31.6 %, Bright 7.5 / 27.8 % at 1×/2×.
  References (same recogniser): espeak-ng 5.9 / 63 / 89 %, flite 7.5 % at
  1×; our 1× audio time-compressed (WSOLA) 41 / 86 / 96 % at 2/3/4×, so
  the 4× number is the recogniser's limit. Budgets in `budgets.toml
  [tts]` (18 / 36 / 97 %); the test reports "not run" without
  `ORPHEUS_ASR_URL`/`ORPHEUS_ASR_CMD`. Tuning by measurement: levels of
  fricatives, nasals, /h/ and bursts, source pre-emphasis and F6, liquids
  holding their targets, abrupt onsets in stop clusters (1×: 15.4 →
  13.9 %, 2×: 29.8 → 30.5 %, within the run-to-run noise of my tuning).
* **Tests**: `tests/speech.rs` (13: exact durations at 4 speeds × 3
  rates, word-event samples, block-size and feeding independence, LPC
  formants, fricative spectra, F0 contours on audio and on `pitch()`,
  pitch across speeds, no clipping/NaN, events, both stops, RAM, every
  phoneme), `tests/golden_speech.rs` (34 hashes, `tests/goldens/
  speech.txt`), `tests/intelligibility.rs`, unit tests; fuzz target
  `tts_synth` (6 seeds in `tests/fuzz-corpus/tts_synth`, one regression
  input in `tests/regress/tts_synth`: the harness's own limit of 10⁶
  render calls hit by a legitimate one-minute render in 1-frame blocks —
  the limit now follows the expected length), 4 min of
  libFuzzer + ASan (2,793 inputs): no findings. A restructuring of the
  signal path into per-filter block loops (spills 120 → 7 memory
  operations per sample) left every golden hash unchanged.
* **Gate**: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`
  and `cargo test` on `orpheus-tts`: pass; workspace-wide `cargo fmt
  --all --check`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test --workspace` (167 suites): pass; `cargo +nightly-2026-11-05
  fuzz build tts_synth`: builds. **Not run**: `xtask test all`, `xtask build`/`size`/
  `check-resident`/`stack` (no firmware change), `xtask dev …` (not
  allowed for this package).
* **Open / for the lead and the owner**: (1) the owner's check for M15
  TTS-2: listen to the voices and rate them (WAVs from the `say`
  example); (2) the Base CPU budget at 44.1/48 kHz (governor step or a
  shorter resampler kernel) and the 64 KiB code budget need a decision;
  (3) firmware integration items at the end of 04 §8 "as built (TTS-2)".
### 2026-11-25 — GAME-4/5: Salvo, the artillery duel (host and simulator; worker, no hardware)

Built on the host and in the simulator; **not in the firmware** (feature
`salvo`, off there: it does not fit the stack budget, numbers below).
Design, rules, keys, settings and numbers: 05 §7.13 "As built (GAME-4/5)".

* **Engine** `orpheus_games::salvo` (feature `salvo`, 680 B): height-map
  world 128–384 px of five terrain styles (+ mixed) with roughness, relief
  and a terrain seed; craters whose earth above falls in, dirt mounds,
  tanks that fall (fall damage), slide and sink; SI physics at 200 Hz
  (semi-implicit Euler, 20 steps per 60 Hz step, game time 6×), gravity
  presets, quadratic drag relative to the air (wind and gusts act through
  it), mass and diameter per weapon; seven weapons of its own (shell,
  heavy, cluster, roller, digger, dirt bomb, smoke) and a shop; 2–4
  players, pass screens, rounds, scores and credits; computers of four
  difficulties (wind noise and a per-round bias, bisection over angles
  with the same physics, bracketing from the last miss, aim noise; Expert
  sees the gusts by copying the generator, the others never). One
  simulated shot per logic step, at most one per frame.
* **Physics checks** (`crates/orpheus-games/tests/salvo.rs`): no drag
  against the closed form (x exact to 1e-4; y with semi-implicit Euler's
  `g·Δt·t/2` lag, within 0.1 m); drag without gravity against
  `v₀/(1 + c·v₀·t)`, `ln(1 + c·v₀·t)/c`; drag without wind: the fall
  `v_t·tanh`, `v_t²/g·ln cosh` and the shot straight up (apex time
  `v_t/g·atan(v₀/v_t)`, height `v_t²/2g·ln(1 + v₀²/v_t²)`) within 0.3 %;
  wind only through drag (no air: bit-identical in any wind); energy per kg
  never grows without wind (300 random game shots, 1e-4); the top-speed
  table reaches 1.25 widest worlds (and not 15 % more); the square root and
  the sine table against f64. The thumbv7em build has no fused
  multiply-add (0 `vfma`); a scripted three-computer game with gusts is
  pinned by checksum (`deterministic_and_pinned`, 0xd5cb_f752).
* **AI**: Expert hits a still target in calm air within two shots on every
  gravity × air × width × style tried (144 cases, ≥ 90 % on the first);
  average shots to hit in light wind: Novice > Easy ≥ Hard ≥ Expert
  (`difficulty_orders_and_bracketing`); Expert hits within three shots in
  strong gusts (it sees them); the search's simulation equals the flight.
* **Invariants**: `Salvo::check` (no NaN, no flying shell in the ground,
  heights within 0..=104 and zero beyond the world, tanks inside, health
  within the setting, view inside) after every input and step of 60 random
  games and in the fuzz target `salvo_keys` (also: health never rises in a
  round, saves read back, determinism); 6 seeds in
  `tests/fuzz-corpus/salvo_keys/`, replayed by `tests/fuzz_corpus.rs`;
  `cargo fuzz` 122 s: 1,160 runs, no finding.
* **App and simulator**: `Game::Salvo` (Games list fourth), screen `Salvo`
  (`screens/salvo.rs`), settings in three groups (Players, World, Rules)
  of the pause menu's Settings (`Game::groups`, `GameListScreen::group`);
  21 settings `0x0E61–0x0E76` packed into four stored words
  `0x0E80–0x0E83` (new `Setting::pack`: get/set per field, the cache's
  `STORED` 120 → 124 stays within 128; `packed_settings` test), VER-13
  manifest entries for all 25 keys, differential tests of every setting
  (`sim/tests/games.rs::salvo_settings`), `salvo_saved_and_continued`;
  simulator hooks `set:salvoturn|aim|wind|health|flat|place|blast`;
  `goto:Salvo`. New `Unit::MetresPerSecond`.
* **Goldens**: 53 new `games_salvo_*` (aim, menu, overview, flight, fast,
  blast, crater, pass, thinking, round over, shop, game over, mountains,
  canyon, islands, plateaus, 384-px overview with four tanks, smoke,
  preview, timer, pause, settings groups and each group, help; dark and
  light; four tanks in Amber) and the Games list (4 changed: the Salvo
  row). Looked at all; fixed from looking: dim smoke and guide dots, the
  shop's columns, arrows for tanks beyond the view.
* **Suite**: the `games` journey's list shot masks the fourth row (Salvo is
  in the simulator, not in the firmware); `expected/games/list.png`
  re-made with `ORPHEUS_BLESS=1` through the test (not `xtask dev`).
* **Measured** (host, release): a simulated shot ≤ 0.43 ms (Moon, 384 px),
  a decision 0.4–2.0 ms, 25 ns per physics step, 1 µs per flight logic
  step; game loop per frame: aiming 10.4 µs / 21 dirty rows, a flight
  14.6 µs / 38 rows, four computers 27 µs / 89 rows. Player estimate: a
  simulated shot ≤ 20–25 ms, one per frame (the UI never waits for more);
  the aim guide's last-shot path is one simulation per redraw.
* **Firmware RAM and stack** (`cargo xtask build`, `size`, `stack`):
  - with Salvo: Session 352 → 680 B, hub 848 → 1,176 B, `.bss` +368 B,
    resident +368 B (constant pools and string literals: mergeable
    sections stay resident), the stack's start three 256-B steps up
    (usable 20,480 → 19,712 B); the UI task's deepest path grew 516 B
    (new game → an answer → decode, 680-B copies) and is back to 6,796 B
    with the in-place construction and decoding: **margin 3,604 B, budget
    4,096 B — does not fit.** It needs ≈ 490 B: a cut of the UI task's
    deepest path (the Authenticator's `Fw::derive_finish`, a 4,008-B
    future frame) or ≈ 460 B less static RAM (e.g. the 512-B card buffer
    lent from elsewhere). Then `orpheus-apps/salvo` on in the firmware and
    the suite's mask removed.
  - as committed (no Salvo): resident 80,747 B, `.bss` 124,864 B, usable
    stack 20,224 B (resident +72 B from shared code crossed a step),
    thread context 6,900 → 6,796 B (the new-game and save-file paths no
    longer copy a session through the UI task's frames): **margin 4,116 B
    (budget 4,096)**; `check-resident` ok.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings` (and `orpheus-apps` without features, with `games`, with
  `salvo`; `orpheus-games` with and without `salvo`), `cargo test
  --workspace` (1,358 passed, 9 ignored), goldens; fw `fmt --check`,
  `clippy --workspace -D warnings`, `cargo xtask build`, `size --check`,
  `check-resident`, `stack --check`: pass. Not run: `xtask test all`
  beyond these, every `xtask dev` step (the lead owns the player).
* **Lead, on the player**: nothing of Salvo yet. The games build changed
  in two ways to check: the game-switch pieces (entry below) and the
  in-place new game / save read (play each game, switch games during
  music; Continue after a reboot). Decide how to make room for Salvo
  (above).

### 2026-11-25 — games card operations in pieces while music plays (worker, no hardware)

Lead's measurement: with an MP3 playing, a game switch (a save written,
then another game read) was one `serve:games` piece of 100 ms
(`chore: starved serve:games 100/26 ms`, `ub=serve:games:100ms`, 8
underruns): above every chore budget (≈ 42 ms), forced after the 2 s
starvation bound.

* `orpheus-fs`: `Volume::flush_step` (the FSINFO update and one dirty
  sector written back with its FAT copies, or, when none is left, the sync
  and the clean flag; the image after the steps equals `close`'s, fsck
  clean: `tests/flush_step.rs` on FAT16, FAT32 and exFAT 128 KiB) and
  `Volume::write_entry` (a file's entry into the sector cache).
* `orpheus_apps::games::CardRun` (host-tested, 8 bytes, no file handle
  kept between steps): every `CardOp` in steps of one path walk with a
  read, or a truncating open with the write into the sector cache; one
  sector written back; the sync. `piece(vol, buf, more)` runs steps while
  `more()` allows (the firmware passes `audio::piece_left`).
* Firmware: `Req::Games` starts the run and does its first piece in the
  request's chore; the rest are listing-job pieces of a new kind
  `job:games` (`media::JOB_KINDS` 6 → 7, seed need 15 ms; the request's
  seed 25 → 15 ms), stepped ahead of other jobs. Idle (power-off, USB
  hand-over, no music) a run finishes in one piece, and the idle loop
  finishes a run left by playback **before** a pending card hand-over or
  power-off. The UI's queue semantics are unchanged (one operation out; a
  game in progress written before another is read).
* Simulator: the deferred mode (`set:gamesio=on`) runs the same `CardRun`,
  one step per simulator step.
* `regress_games_card_ops_in_pieces` (`orpheus-apps`, games): every
  operation on exFAT 128 KiB and FAT32 images with a budget of one step
  per piece: at most 3 sector writes and 6 sector I/Os per piece (making
  `/.orpheus/games` the first time zeroes its cluster: once per card),
  the same answers and files as in one piece. Measured steps (exFAT, warm
  sector cache): Load 4 pieces (with Salvo 5), Write 5 pieces of ≤ 2
  sector writes, Read 1 piece (0–2 reads), Remove 4 pieces; FAT32 Write 6
  pieces of ≤ 3 writes (its truncating open writes the dirty flag into
  both FATs).
* **Expected on the player** (lead to measure): `job:games` and
  `serve:games` pieces of one sector write and a card busy wait each,
  ≈ 3–15 ms, the last one (sync and clean flag) the longest; a game
  switch ≈ 5 + 1 pieces spread over successive chores instead of one of
  100 ms; no `starved serve:games`.
* Firmware: resident 80,675 B, bss 124,864 B (+16 B: the run in the card
  task's server), stack margin 4,268 B unchanged.

### 2026-11-25 — on-device suite on the games build: 16/16 (lead)

* Build `40953ef7` (Games on Home, Solitaire, Recordings fix): 16 journeys
  (new `games`); stack free ≥ 7,824 B of 20,832 B. Three shots needed
  masks, each with its reason in `tests/suite/journeys.toml`: `about` (the
  full-layout player's "Firmware slot" row, absent in the simulator) and
  the USB group's first row (`usb_group`, `settings/usb`: the focused
  label "When connected" scrolls and the reads land mid-scroll — open: the
  marquee's rest against the 6 s retry). After that: **16 of 16 pass**.

### 2026-11-25 — Recorder › Recordings fixed; the lead's test recordings removed (lead)

* Recorder › Recordings… listed the **card's root** instead of
  `/Recordings`: the screen sends the folder's id, and `Fw::entry_of`
  resolves ids only through the current browse window (empty when coming
  from the Recorder), so the card task was sent "no entry" = root. Fix:
  `rec::find_dir` (and the first recording) also keep `/Recordings`' own
  entry, and `entry_of` answers it (also for "Play folder"). Deleting from
  the root view would have been refused (the card task only deletes inside
  `/Recordings`). On the player: the screen lists REC-0001.wav (102 MB) and
  REC-0002.wav.
* After a delete the screen showed the deleted file: the firmware answered
  the screen's re-browse from its cached listing. Fix: a queued delete marks
  the listing incomplete. Not verifiable by a test without recording into
  `/Recordings` (rule 5): user check 29 (record, delete, the list updates).
* With the owner's approval ("delete files, keep folder"): REC-0001.wav and
  REC-0002.wav (the lead's 2026-11-24 test recordings) deleted through
  Recorder › Recordings › Delete; `/Recordings` kept, now empty.
* Soak seed 3 passed (39.2 min, 24 actions, 0 underruns); `ls` of some
  large root folders gets no answer within 18 s (`/KEXP`, `/Tinydesk`,
  `/Vulfpeck`) — the developer `ls` should answer big folders (tooling).

### 2026-11-25 — soak start-up hang: the command timeout counted only sleeps (lead)

* A second soak (seed 3) sat after "player back" for over 28 min, like
  the earlier one; the player itself was healthy. `player::command`'s
  timeout counted only its 100 ms sleeps, not the time each reply poll
  took, so a player answering slowly stretched a 15 s listing wait into
  minutes, over up to 40 listings. Now bounded in real time too
  (`regress_command_timeout_is_bounded_in_real_time`); slow or failed
  listings during folder discovery are logged.
* `/Recordings/REC-0001.wav`, `REC-0002.wav` on the card are the lead's
  recordings of 2026-11-24 (`rec start mic wav 10` on `d7f59d7c`, before
  developer recordings went to `/.orpheus/test/rec/`) — outside
  `/.orpheus/` (rule 5). Asked the owner about removing them.

### 2026-11-25 — soak seed 1 clean on the merged build (lead)

* Build `ac68421b` (stack headroom, settings commands, games behind a
  feature, synth crate): `dev soak mixed --minutes 30 --seed 1` **PASS**,
  35.3 min, 27 actions, **0 underruns** (none even in disruption windows),
  DAC headroom ≥ 571 frames, stack free ≥ 9,616 B (static worst case
  16,196 of 22,272 B), uptime drift −20 ppm.
### 2026-11-25 — TTS-1: `orpheus-tts` front end (worker, no hardware)

New crate `crates/orpheus-tts` (`no_std`, no `alloc`, `forbid(unsafe)`, no
dependencies; "as built" in 04 §8). Not wired into the firmware, the
simulator or `orpheus-apps`.

* **API**: `Converter::new(Options { spell, lines }, Speed)`, `feed` /
  `finish` / `next_token` → `Token::Word { pos, phrase_final, spelled }`,
  `Token::Phone(Seg { phone, ms })`, `Token::Break(Pause { kind, contour,
  ms })`; `word_text()`, `convert(text, f)`, `reset`, `set_speed`.
* **Measured**: converter RAM 1,408 B (host); thumbv7em opt "s" text
  31.5 KB + rodata 50.2 KB (lexicon 32,766 B, paged as data in TTS-2);
  host 4.4 µs/word. Accuracy against CMUdict on 6.8 M Gutenberg tokens:
  rules alone 39.3 % of words / 79.8 % of tokens (phonemes), 35.6 % /
  78.5 % with primary stress; rules + lexicon 98.1 % of tokens, top 5,000
  words 99.8 %, every normaliser and interface word right. Curve in
  `crates/orpheus-tts/data/lexicon-report.txt`. 10× setting ≈ 7.7× faster
  than 1× in practice (duration floors).
* **Tool**: `cargo xtask tts-lexicon [--check] [--budget B]` downloads
  CMUdict (pinned, SHA-256) and 57 Gutenberg books (from the
  `aleph.pglaf.org` mirror: gutenberg.org answered 503) into
  `target/orpheus/tts-data/`. The committed files were produced by the same
  `orpheus_tts::lexgen::run` through a scratch driver; the xtask binary
  itself was clippy-checked but not run (disk: see below) — **lead: run
  `cargo xtask tts-lexicon --check` once**.
* **Tests**: unit, `normalise` (166 forms), `goldens` (143 phoneme strings,
  duration tables at 0.25/1/4/10×), `accuracy` (thresholds), `properties`
  (feed-size independence, arbitrary UTF-8/bytes, ≤ 24 tokens per byte,
  duration shape), fuzz targets `tts_text`, `tts_lexicon` with 9 seeds
  (`tests/fuzz-corpus/tts_*`), added to `xtask test fuzz-seeds`. The fuzz
  seeds found an endless loop in the lexicon decoder on truncated data
  (fixed).
* **Plan fix**: 04 §8 said the speed has 39 values; 0.25–10 in 0.25 steps
  is 40.
* **Licences**: CMUdict notice in `crates/orpheus-tts/data/LICENSE-CMUDICT`
  and `THIRD-PARTY.md`; its clause 2 (binary redistribution reproduces the
  notice) applies to firmware images once TTS ships — **lead: carry the
  notice with release builds** (as for the other BSD parts).
* **Gate**: `cargo fmt --check` (crate), `cargo clippy -D warnings` on
  `orpheus-tts` (with and without `build`, all targets) and on `xtask`:
  clean; `cargo test -p orpheus-tts` (all suites, with and without
  `build`): pass; `cargo +nightly-2026-11-05 fuzz build` of both targets:
  builds; `tts_text` fuzzed 60 s (15 k runs) and `tts_lexicon` 45 s
  (1.46 M runs): no findings. **Not run**: workspace-wide clippy/test and
  `xtask test all` — the disk had 0.9–2.6 GB free while other workers
  built. Open: none for the owner; TTS-2 needs the items listed at the end
  of 04 §8 "as built".
| GAME-4/5 Salvo (M14) | **built (host + simulator + firmware), not yet run on the player** (feature `salvo` on in the firmware since STACK 2026-11-25: stack margin 4,560 B, budget 4,096) | Artillery duel: terrain styles, destructible ground, SI physics with drag and wind, seven weapons, shop, 2–4 players, computers of four difficulties; 21 settings packed into 4 words; 53 goldens; fuzz `salvo_keys`. Games card operations now in pieces while music plays (lead's 100 ms `serve:games`). Lead: session entries 2026-11-25 "GAME-4/5" and "games card operations in pieces" |
| GAME-2 + Games on the player (M14) | **built (host + simulator + firmware), not yet run on the player** | Solitaire (Klondike: draw 1/3, passes, standard/casino/none, auto-move, auto-complete, exact undo, best-effort winnable deals; fuzz target `solitaire_keys`, 33 goldens, 7 settings `0x0E41–0x0E47` with differential tests); firmware `GamesService` (hub 872 B + 512 B buffer, card task `Req::Games` as a chore while playing, saves on pause, quit, power-off and USB hand-over), Games on Home (default mask 0x0FFF; the owner's stored mask needs the Games bit). Stack margin 4,268 B (budget 4,096). Lead: session entry 2026-11-25 "GAME-2" |

### 2026-11-25 — GAME-2: Games on the player; Solitaire (host, simulator, firmware) (worker, no hardware)

Two parts: the Games app (GAME-0/1/3) integrated into the firmware, and
GAME-2 Solitaire. 05 §7.13 has a second "As built" note (rules, keys,
layout, scoring, files, settings, numbers).

**Firmware integration** (built, not yet run on the player):
* `orpheus-apps/games` on in `fw/firmware` (and `orpheus-ui/menu-key`
  with it); `App::Games.on_player()` true; the default Home mask is every
  app (`settings::HOME_ALL` = 0x0FFF): Home has twelve cells, Games before
  Settings. **The owner's player has 0x07FF stored** (restored with
  `settings default` on 2026-11-25): Games appears only after Settings ›
  Display › Home screen › Games, or `settings default 0x30b` (the lead's
  call). The test look (suite) reads the default, so the suite sees Games.
* String table: the game strings are no longer behind the feature (the
  numbers no longer depend on it); new strings (Solitaire, 23) are
  appended after them.
* `GamesService` (`fw/firmware/src/games.rs`, `services.rs`): the hub in
  `GamesRam` (872 B, `.bss`, written once by `Fw::new`; a `const` hub
  landed in `.data` first: +1.4 KB resident) and a 512 B card buffer
  (`static mut`, reached through raw pointers by one side at a time).
  Requests become card operations in `orpheus_apps::games::GamesIo`
  (host-tested `games_io_queue`; the simulator's `set:gamesio=on` runs the
  same queue, `deferred_card_queue`): Load (read `scores.bin`, look for
  the three save files), Read, Write (truncate, write, close, flush),
  Remove, WriteScores; one out at a time as `media::Req::Games` (request
  kind 15, `REQ_KINDS` 16) with the buffer lent (slot protocol as the
  Authenticator's import: `games::SLOT`, data tag 0x101). Idle: served at
  once. **While music plays: a chore of its own kind under the chore rule
  (`audio::SEED_US` 25 ms until measured)**. `Open` is answered late: the
  hub's `opening` names the game, the game screen shows "Loading…" until
  the save is read (Back leaves); a game in progress is written before
  another is read; a read nobody waits for is dropped; the score table
  is never written before it was read. A job not started in 3 s is taken
  back; without a mounted card operations fail at once (a read starts a
  new game; nothing is marked saved).
* Seeds: uptime (µs, with a counter) and the RTC's seconds since 2000,
  mixed (`seed_from`). `display_on`: the screen state the UI task
  publishes (`drain::Shared::screen_on`).
* Saves: pause and quit (as before), **power-off** (`Ui::power_off`, after
  the recorder, before the card is flushed: up to 1.5 s) and **before
  the card goes to a computer** (USB file transfer or disk image: up to
  1 s before the USB task is signalled, so the save is ahead of
  `Req::Release` in the card task's queue).

**Solitaire (GAME-2)**: `orpheus_games::solitaire` (Klondike: draw one or
three, passes unlimited / 3 / 1, standard / casino / no scoring,
safe auto-move also at the deal, auto-complete, a 100-move exact undo,
game over when no move changes anything, a best-effort winnable-deal
check), the Solitaire screen (`screens/solitaire.rs`: 128 × 128 layout,
seven 18 px columns, 17 × 20 px cards, own 3 × 5 ranks and 5 × 5 / 7 × 7
suit shapes that differ by ≥ 3 pixels, fans closing up when long, 2 px
cursor frame, partial redraw per pile), cursor model and keys per §7.13,
seven settings `0x0E41–0x0E47` (`STORED` 113 → 120), casino bank in the
score table (`Scores::bank`, variant 0xFFFF). The Games list has
Solitaire third.

**Tests**: engine unit tests (9); `tests/solitaire.rs`: 300 random games
and every rule combination through the fuzz body, winning lines of the
deal check played with the keys (≥ 10 won games), assists, keys, save and
continue seamless, fixed deals, the deal check's effort; fuzz target
`solitaire_keys` (every recorded move checked against an independent
reference model of the piles and rules, the board equal to the
reference's, undo back to exactly the position before the move it takes
back, score rules, saves read back, determinism) with 4 seeds in
`tests/fuzz-corpus/solitaire_keys/` (one a won game), replayed by
`tests/fuzz_corpus.rs`; 120 s of `cargo fuzz` (nightly-2026-11-05): 2,955
runs, no finding (slow: every input twice, every move checked).
Simulator (`sim/tests/games.rs`): Solitaire saved/undo/Back, **partial
redraw equals a full draw after every key** (found and fixed: blended
frame corners over stale gap pixels, a target mark not keyed), the
deferred card queue, casino bank, and **differential tests of all seven
settings** (own game changes, the other two games and Home do not);
`games::tests::games_io_queue`, `casino_deal_charged_and_paid`.

**Measured** (host): deal check, release: 0.93 ms per new game on average,
2.89 ms worst (draw one, every new game checked); 2.20 / 4.60 ms (draw
three, 75 % checked); 1.31 / 1.72 ms (draw three, one pass: the check
rarely wins, the first deal is used). Effort was cut from 24 deals × 6
play-outs to 16 × 4 for the player. Player estimate (× 100–150 for an
M4 at 96–144 MHz, code paged): 0.1–0.5 s per new game, the UI waiting
(no "Dealing…" frame; a user check). Game loop (simulator with every
service, per 33 ms frame): Home 2.9 µs; Blocks falling, a key every
200 ms: 7.1 µs and 28.6 dirty rows; Solitaire with cursor keys 13.9 µs
and 15.2 rows; Solitaire's timer alone 3.1 µs and 0.3 rows. Blocks
already ran at 30 frames/s (`TICK_MS` 33) with partial redraw (well and
panel keyed): unchanged.

**Firmware numbers** (`cargo xtask build`): resident 80,620 B (text
27,896, rodata 36,351, data 16,372), `.bss` 124,848 B (VER-10's record:
resident 80,108, bss 123,408); code pages 113/128. **Static stack: 16,212
B nested worst case of 20,480 B usable, margin 4,268 B** (was 6,076 of
22,528): static RAM +1.9 KB (hub and queue 872, buffer 512, settings cache
+22 keys) moved the guard eight 256 B steps; `__bss_end` 0x1322A0 is
96 B below the next step. Thread context 6,876 → 6,900 B, rt 8,144
(unchanged). `budgets.toml` `stack_static_margin_bytes` 5,820 → **4,096**
(the VER-10 goal: the next static step needs a stack cut first).
Alternative kept in reserve: lend the frame buffer's last two rows as
the card buffer (−512 B static, two steps) at the cost of no drawing
while a save is out (up to 300 ms while music plays).

**Suite and goldens**:
* `tests/suite/journeys.toml`: journey `home` — `next*6` from Recorder
  now reaches Games: new shot `home_games`, then `next shot:home_settings`;
  expected `home.png`, `home_menu.png`, `home_recorder.png`,
  `home_settings.png` redrawn (twelve-cell grid), `home_games.png` new.
  New journey `games` (Home › Games list, values masked: they follow what
  was played; reads only) with `expected/games/list.png`. Other journeys
  unchanged (Settings is still `prev` from Music; the others are within
  the first ten cells). `xtask dev suite`'s Home frames: one per app
  (`App::COUNT`).
* Soak: `App::Settings`'s Home position follows `App::GRID` (10 → 11:
  **`browse settings` would have opened Games**); new explicit step
  `browse games N SEED` (random keys in the Games app over the music; not
  in the random picks, so `mixed` seed 1 replays as before).
* Goldens: 36 changed — Home (`app_home_*` 6, `app_home_playing_*` 6,
  `radio_home_*` 6, `tools_indicators_*` 6, `app_accent_red_home`,
  `app_amber_home`: the Games cell), Settings › Display › Home screen
  (`app_home_apps_*` 6: a Games row), the Games list (`games_list_*`,
  `games_list_continue_*`: a Solitaire row); 33 new `games_sol_*` (new
  deal, mid-game, depth, held, refused, draw three, long column, auto-
  complete, won, no moves, casino, no score, pause, settings top/end,
  How to play; Dark and Light; mid-game in Amber). Looked at all of them.

**Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
warnings` (and `orpheus-apps`/`orpheus-ui`/`orpheus-games` without
features), `cargo test --workspace` (1,266 passed, 9 ignored), goldens;
fw `fmt --check`, `clippy --workspace -D warnings`, `cargo xtask build`,
`size --check` (resident 80,619 B, bss 124,848 B), `check-resident` (57
sections from 19 roots, none paged), `stack --check` (margin 4,268 B,
budget 4,096 B): pass. Not run: `xtask test all` beyond `cargo test
--workspace`, brand/site checks, HIL and every `xtask dev` step (the lead
owns the player).

**Lead, on the player** (user checks):
* Home shows Games (after restoring the Home mask, above); Games › each
  game: play Blocks, 4096 and Solitaire (draw one and three; casino once).
* Save/continue: Back → pause menu (save written: `/.orpheus/games/<g>.sav`),
  Quit, open again → "Continue" in the list and the same game; open
  another game while one is in progress (the first is written first);
  reboot and continue.
* Power-off during a game (not from a test: by hand) → power on → the game
  continues; USB file transfer during a game → the save is on the card
  the computer sees; back → Continue.
* Music while playing: Blocks during MP3/FLAC playback (no underruns;
  `pcm:` lines; `chore` lines name `serve:games` with its measured need);
  a pause/quit save while playing; `cargo xtask dev soak --replay` of a
  file with `browse games 40 1`.
* Solitaire's new game with Winnable only on: how long the UI waits (the
  estimate above), drawing three.
* The device suite (`home` and the new `games` journey).

### 2026-11-25 — **`fuzz-ui --settings` changed the owner's stored settings** (lead) — fixed

* The on-device suite on the stack-headroom build (`10607b87`): 13 of 15
  journeys pass, stack free 9,872 B (was 6,136). `system/about` differs
  because the player now shows "Firmware slot" (T-6), the simulator's
  reference not: expected drift, the reference needs the row.
  `settings/sound` showed **Equaliser: Electronic** where every earlier run
  showed Flat.
* Cause: `cargo xtask dev fuzz-ui --settings` let random keys change
  settings through the menus, which stores them; it never turned on the
  firmware's RAM-only settings test session (the soak does). The lead's
  report that those settings changes "stay in RAM" was wrong.
* New read-only `settings changed` developer command (lists stored values
  that differ from their defaults; +8 B resident, which crossed one 256 B
  step: stack margin 6,340 → 6,076 B, ratchet 5,820 B). On the player:
  equaliser 5 (Electronic), shuffle 1, repeat 1, brightness 16, accent 1,
  files sort 1, timer presets reordered (300/1500/180/·/60 s), Home apps
  2046, output 1 (Bluetooth, from the lead's Bluetooth tests), radio
  95.0 MHz. **Asked the owner which to restore.**
* **Restored with the owner's approval ("restore all but radio")**: new
  developer command `settings default ID` (stores the default; hex ids in
  lowercase); equaliser, shuffle, repeat, brightness, accent, Home apps,
  output, files sort and the five timer presets back to their defaults;
  after a reboot `settings changed` lists only the radio station
  (d50=9500).
* Fix: every fuzz session and replay runs inside the test session
  (`fuzz::reset_ui` turns it on after each reboot), `--settings` refuses
  firmware without it (`regress_settings_fuzzing_requires_the_ram_only_session`),
  and the suite turns it on too.
### 2026-11-25 — SYN-1: `orpheus-synth` core (worker, no hardware)

New crate `crates/orpheus-synth` (`no_std`, no `alloc`, `forbid(unsafe)`;
spec and the full "as built" in 04 §9). Not wired into the firmware, the
simulator or `orpheus-apps` (SYN-3; the Games worker owns `orpheus-apps`).

**API**: `Engine::new(rate, seed)`, `set_patch(part, &Patch)` (16 parts),
`render_i16`/`render_f32(&mut buf, &[Event])` with `Event { offset, kind }`
(note on/off, part parameters, effect parameters, all-notes/sound off,
tempo, master) applied on their exact sample; `apply`, `set_voice_limit`,
`set_delay/chorus/reverb`, `voices()`, `reset(seed)`. `Patch` is plain
data: subtractive (2 oscillators + noise), FM (2–4 operators), drum, GM
kit; filter, three ADSRs, LFO, glide, mono/legato. Building blocks (`Osc`,
`Svf`, `Env`, `Lfo`, `FmState`, `DrumVoice`, effects) are public for the
signal generator, games and speech. 12 melodic presets and 6 drum voices
(`presets`).

**Determinism**: IEEE basic `f32` operations only, integer phases, seeded
noise; rendering cut at events and at 32-sample control ticks of the
engine's own clock, ramps of fixed length, so the output is independent
of the caller's block sizes (tested 1…4096). The golden hashes match in the
test build (opt-level 1) and in the release build (`cargo run --release
-p orpheus-synth --example render -- --check`: 28 scenarios, 0
mismatches). The player must keep `FPSCR.FZ` = 0 (reset value).

**Looked at, fixed** (renders as waveform + spectrogram PNGs; no
listening possible here): the PWM pad wandered ±0.6 at the LFO rate (the
pulse's DC moved with the width) → zero-mean pulse; voice steals left
broadband clicks with a 64-sample fade → 256 samples in hidden tail slots;
the instant-attack limiter splattered on dense mixes → gain smoothed over
1 ms with a soft clip for the overshoot; the default delay (¾ beat) did
not fit the line → ½ beat. FM: an event between ticks advanced the
operator envelopes twice → they advance on ticks only.

**Measured (host, x86-64, opt "s")**, ns per voice-sample: sine 4.4, saw
3.6, saw + LP 7.2, pulse + LP 8.6, wavetable + LP 9.3, saw-lead 9.8, PWM
pad 11.7, FM 2-op 20.6, FM 4-op 41.7, drums 5.3–24.8; effects per sample:
delay 4.4, chorus 9.7, reverb 19.1; empty engine 10.1 ns/frame. Optimised
on the way (saw-lead 13.5 → 9.8, sine 10.8 → 4.4, FM 4-op 56 → 42):
32-sample envelope steps, cached pitch/cutoff designs, integer-phase sine,
FM graph as a matrix written out per operator count, no division at the
BLEP edges, control period 16 → 32.

**Player estimate** (thumbv7em disassembly, ±30 %): cycles per
voice-sample sine ≈ 75, saw-lead ≈ 140, PWM pad ≈ 190, FM 4-op ≈ 260;
delay ≈ 95, chorus ≈ 120, reverb ≈ 190, engine + limiter ≈ 135 per sample.
16 saw-lead voices at 48 kHz ≈ 114 MHz (79 % of 144 MHz; 93 % with all
effects), ≈ 6 voices at Base (48 MHz): **the plan's "16 voices at Base"
is not met**; SYN-3 measures and sets the clock floor by voice count.

**RAM**: voice slot 328 B, patch 292 B, delay 24.6 KiB (12,288 × `i16`:
256 ms at 48 kHz; 16,384 put the engine at 77 KB, over the 72 KiB
decoder arena), chorus 4.1 KiB, reverb 16.3 KiB, core 23.6 KiB, **engine
69,040 B** (asserted ≤ 72 KiB). Code 38.6 KB, read-only data 22.4 KB
(wavetables 14.3 KB) for the whole crate (thumbv7em, opt "s", before LTO).

**Aliasing** (65,536-point FFT): saw at 1046/2794/4186 Hz −31/−27/−24 dB
of all energy, −51/−44/−43 dB below 12 kHz (naive −16/−12/−10); triangle
−60/−46; wavetables ≤ −47; budgets = measured + ~2 dB. The 2-sample
polyBLEP's residue sits in the octave below Nyquist (the sweep effect at
C9 shows it).

**Tests**: 19 unit tests; `tests/engine.rs` 10 (scheduling over random
blocks, parameter timing, block-size independence, seeds, stealing order,
16-voice cap, mono legato, sustain, SVF stability, RAM table);
`tests/golden.rs` (28 scenarios, `tests/goldens.txt`); `tests/spectrum.rs`;
fuzz target `synth_events` (`crates/orpheus-synth/fuzz`, built with
`nightly-2026-11-05`; 2 min, 17 k runs under ASan, no finding; 4 seeds in
`tests/fuzz-corpus/synth_events/` replayed by `tests/fuzz_corpus.rs`).

Gate: host fmt, clippy `-D warnings` (workspace, all targets), `cargo test
--workspace` (1,163 passed, 9 ignored): pass; the fuzz target builds
(`cargo +nightly-2026-11-05 fuzz build synth_events`) and ran 60 s (18.9 k
runs, no finding); release golden check 0 mismatches. Not run: `xtask test
all` beyond `cargo test --workspace`, brand/site checks, firmware build,
size/resident/stack checks (no firmware change), HIL (no hardware). No
simulator hook (not trivial: SYN-3).

**Open / next**: SYN-2 (drum models, sample playback, kits and presets as
data, longer delay from memory the synth may borrow, listening); SYN-3
(engine in the decoder arena, DAC feed, golden hashes on the player,
cycles per voice into `budgets.toml`, clock floor, wavetables and hot loops
resident or clustered). Plan question for the lead: the 16-voice-at-Base
target (04 §9) against the estimate above.

### 2026-11-25 — VER-10: card task frames, static stack margin 204 → 6,340 B (worker, no hardware)

Trigger: margin 204 B (21,812 B nested of 22,016 B usable) with
`__stack_bottom` on a 256 B step; the goal from VER-10 is ≥ 4 KiB.

**Static worst case** (`cargo xtask stack`, base `182b087d` → this commit):

| Context | Prio | Before | After | Deepest chain after |
|---|---|---|---|---|
| init | thread | 12,368 | 12,368 | `init_chain` 11,192 B frame (RT and USB do not exist yet; nested 13,240 B) |
| thread | thread | 6,876 | 6,876 | UI task → `Fw::derive` → `derive_finish` 4,008 → seal → `Pool::fill` → SHA-256 |
| irq-normal (USB) | L3 | 80 | 80 | |
| rt (card task) | L2 | 13,768 | **8,144** | task poll 120 → `Apart<idle>` 424 → `Apart<play_path>` 200 → `play_file` 344 → `play_block` 208 → `chores` 208 → `Server::step` 744 → `io_session` 408 → `fill_some` 984 → `Index::rows` 584 → `Inner::slot` 344 → `spin_in`(open) 1,504 → `Volume::open` 488 → `walk` 560 → … SD → time driver → panic tail |
| deadline, pager, prof, fault | L1, L0 | 312, 236, 80, 32 | unchanged | |
| **nested, run** | | **21,812 of 22,016** | **16,188 of 22,528** | margin **204 → 6,340 B** |

Next RT chains (from the task poll down, 144 B above): `idle` → `play_path`
8,000; `player_request` → `run_player` → … → `play_file` (the same
`fill_some` chain) 8,000; `idle` → `library::slot` → `library::step`
7,928; `release` → `mount` 7,880; `start` → `open_card` → `mount` 6,920;
the Vorbis seek (`play_file` → `seek_to` 1,488 → set-up 2,576) 7,768.
They are within ~250 B of each other: one more cut there buys little.

**What made the frames** (`llvm-objdump`: the `subw sp` and the
`__aeabi_memcpy` sizes around the calls; future sizes from the DWARF):
values moved by value through the async functions, each copied through the
poll function's frame (often twice), and that frame is under every chain
of the function.

* **Audio task poll 2,328 → 120 B**: the volume (736 B) came out of
  `start`'s output, went into `release`'s future and came back out of it.
  Now `vol: Option<Volume>` lives in the task's future; `start(&mut vol)`
  and `release(&mut vol)` fill the slot, the loop borrows it.
* **`idle` 2,064 → 424 B** (`Apart<idle>::poll`, the closure is inlined
  into it now): every child it awaits (`step`, `serve`, the logs, the
  alert, `build_slot`, `count_free`, the library, pictures, hand-over,
  drive map, recorder, `resolve_dir`, `ls`, `play_path`, `create_dir`,
  `upload`) is behind `Apart`, so its temporaries are in a frame of its
  own (the deepest was the developer `play PATH`: `idle`'s frame sat above
  `play_file`).
* **`play_file` 2,320 → 344 B**: the decoder (976 B with its stream
  information) was moved out of `orpheus_codecs::open`'s `Result` twice.
  Now `let mut opened = open(…); let Ok((dec, info)) = &mut opened`: it
  stays where `open` wrote it (in the future) and is used by reference.
* **`start` 2,392 → 56 B** (it became the deepest after the above): the
  card (608 B) and the volume (744 B) came out of `Card::open` and `mount`
  by value; that is `open_card(&mut vol)` now (2,728 B, on a 6,920 B
  chain), and `start`'s and `load_state`'s other awaits are behind `Apart`.
* `stack.toml`: `firmware::audio::idle::{closure#0}` removed (inlined into
  `Apart<idle>`, covered by `<firmware::audio::Apart<*`).

Behaviour: unchanged (the same calls in the same order; `Apart` only gives
a future's poll its own function). No logic moved, so no new host tests;
the regression guard is `stack --check` with the higher ratchet.

**Static RAM**: the same moves shrink the futures — the audio task's pool
7,336 → 6,824 B (its future 7,296 → 6,784: `start` 5,856 → 5,144,
`release` 5,848 → 5,112), so `.bss` 123,920 → 123,408 B, `__bss_end`
0x131B00 → 0x131900, `__stack_bottom` 0x131D00 → **0x131B00** (usable
22,016 → 22,528 B). The bottom is again exactly on a 256 B step: the next
static byte moves the guard up 256 B (usable 22,272 B, margin 6,084 B =
the new budget, still passes). Resident 80,108 B unchanged; paged code 108
pages; `play-block` cluster 15,056 → 14,834 B (15 lines).

**Not done**: the thread side. `derive_finish` (4,008 B: the 1,800 B
`Authenticator::finish` future under `block_on` plus the job's moves) is
the UI's deepest by only 548 B over `Ui::settle` (6,328) and
`Shell::event` (6,232); cutting it means passing the key-derivation job by
reference through `orpheus_otp` (vault API change) for ≤ 548 B.
`init_chain`'s 11,192 B (init phase only) is not limiting.

**Budget**: `stack_static_margin_bytes` 0 → **6,084** (measured 6,340 less
one 256 B step).

Gate: host fmt, clippy `-D warnings`, `cargo test --workspace` (1,130
passed, 8 ignored); fw fmt, clippy `-D warnings`, `xtask build`,
`check-resident` (57 sections from 19 roots, none paged), `size --check`
(resident 80,107 B, bss 123,408 B), `stack --check` (margin 6,340 B,
budget 6,084 B): pass. Not run: `xtask test all` beyond `cargo test
--workspace`, brand/site checks, HIL (no hardware).

**Lead, on the player**: the 30-minute `mixed` soak, seed 1 (expect stack
free ≥ the 8,528 B of the last run; the task poll is in `play-block` and
smaller); a Vorbis seek and a resume into a track (`play_file` →
`seek_to` on the decoder kept in place); a gapless join MP3 → MP3 and a
rate change (`End::Again`); a card hand-over to USB and back (Card reader
or Drive: the volume leaves its slot and is remounted into it; a failed
remount ends the task as before); the developer `play PATH` and `playdir`;
the Authenticator (unlock, codes, change PIN: unchanged code, the thread
side is the deepest context left).
| VER-10 (static stack) | **tool done (host-tested), gate step; firmware cut from ≥ 30,232 to 22,900 B nested worst case (built, not yet run on the player)** | `cargo xtask stack [--check] [--root P]`, `stack.toml`, `budgets.toml stack_static_margin_bytes` (−1,536 B: a ratchet, the static worst case still exceeds the 22,016 B stack by 884 B; the rest is the audio task's own frames, AU-PERF). Lead: `fuzz-ui --replay` of the VER-8 stack sequence, suite stack line (session entry 2026-11-25 "VER-10") |
| GAME-0, GAME-1, GAME-3 (M14) | **built (host + simulator); not in the firmware** | `orpheus-games` (fixed-step framework, SplitMix64, `OGS1` save envelope with CRC-32, score table, Blocks, 4096), `orpheus-apps` feature `games` (Games list, Blocks, 4096, pause menu, settings, How to play; `GamesService`), simulator back end with files under `/.orpheus/games/` (`set:games=on` puts Games on Home), 68 goldens, fuzz targets `blocks_keys`/`tiles_keys`, `tests/settings/effects.toml` (created with the 15 game settings). Firmware integration waits for stack headroom: session entry 2026-11-25 "GAME-0/1/3" |

### 2026-11-25 — GAME-0/1/3: Games framework, Blocks and 4096 (host and simulator) (worker, no hardware)

Package M14 GAME-0 (framework), GAME-1 (Blocks), GAME-3 (4096), host side.
05 §7.13 has an "As built" note with the rules, keys, files and settings.

* **`crates/orpheus-games`** (new; no_std, no alloc, no floats,
  `forbid(unsafe_code)`): SplitMix64 `Rng` (saved with the game);
  `StepClock` (UI ticks → whole 60 Hz steps; a gap over 250 ms pauses);
  `Input` (press, release, long of six game buttons); save envelope
  `"OGS1" | kind | version | len | payload | CRC-32` with validating
  decoders; `Scores` (24 records game × variant: best, detail, played,
  won); `Session`/`Hub`. **4096**: 4–6 boards, goal 2048/4096/8192/endless,
  odds of a 4, 0–3 undo steps (generator restored), keep going.
  **Blocks**: 10 × 20 + 2 hidden rows, bag of 7 or random, own box
  rotation with a kick list (on/off), hold, 3 previews, ghost, lock delay
  with 15 resets, DAS/ARR on logic steps, hold-Down hard drop of the
  piece it was pressed on, levels 1–20 (own gravity table), own scoring
  (100/250/450/800 × level, combo, soft 1 / hard 2 per row), 12-step row
  flash, modes endless / 40 lines / 2 minutes. RAM: `Hub` ≤ 848 B
  (`Session` ≤ 352, `Scores` ≤ 488; size test in `lib.rs`).
* **Tests**: 30 unit tests; `tests/tiles.rs` (300 random games and every
  configuration through the fuzz body: slide = an independent reference
  slide, sum grows by the new tile only, one merge per tile, over exactly
  when no slide changes the board, undo restores; save/continue seamless;
  deterministic replay seed `0x4096` → score 2,436, 211 moves, largest
  tile 128); `tests/blocks.rs` (150 random games, every configuration, a
  greedy player's games in all modes: no overlap, never outside the well,
  no full row left, level follows lines, lines and score = an independent
  reference replay of the locked pieces; deterministic replay seed
  `0xB10C` → 300 pieces, 118 lines, 104,018 points); fuzz targets
  `blocks_keys`, `tiles_keys` (`crates/orpheus-games/fuzz`, same bodies,
  each input run twice for determinism), seeds in
  `tests/fuzz-corpus/{blocks,tiles}_keys/`, replayed by
  `tests/fuzz_corpus.rs`.
* **App** (`orpheus-apps` feature **`games`**, off in the firmware; the
  simulator, browser build and UI fuzzer have it): Games list, Blocks
  (full screen), 4096, pause menu (Resume, New game with a question while
  a game runs, Settings, How to play, Best, Quit), settings, How to play.
  15 settings `0x0E01–0x0E0A` (Blocks), `0x0E21–0x0E25` (4096) in
  `games::SETTINGS` (part of `settings::all()` with the feature;
  `STORED` 98 → 113). Game strings are at the end of `Str` behind the
  feature (the `strings!` macro takes `#[cfg]`); new non-game strings go
  above them while the firmware lacks the feature. `App::Games` (bit 11;
  not in the default Home mask, `App::on_player` false) and `App::GRID`
  (Home order, Settings last). `GamesService` (hub, requests Load / Open /
  Save / Discard / SaveScores, seed, `display_on`). Toolkit:
  `Wants::menu_key` (Blocks' Menu = hold), honoured by the shell only
  with `orpheus-ui` feature `menu-key` (it cost the firmware's key
  handler 8 B of stack: margin 204 → 196 B; gated, back to 204 B).
* **Simulator**: `sim/src/games.rs` (files `/.orpheus/games/blocks.sav`,
  `4096.sav`, `scores.bin` on the simulated card through `orpheus-fs`;
  `set:games=on` puts Games on Home — off by default, as on the player, so
  the device suite's Home frames stay the player's; `set:gameseed=`,
  `set:tiles=`, `set:tiles6=`, `set:blocksrows=`). Keyboard:
  `cargo run -p orpheus-sim --features window -- --window --script
  "set:games=on"` (arrows, Enter = OK/turn anticlockwise, Esc = Back/pause,
  M = Menu/hold). `sim/tests/games.rs`: saved, quit, continued and read
  back after a restart; game over → `scores.bin`, save removed; Menu holds
  and opens no menu; screen off, a notice and the pause menu (only Resume
  plays on); New game asks; and the **differential tests of all 15
  settings** (own game changes, the other game and Home do not).
* **Goldens**: 68 new `sim/golden/games_*.png` (list, continue, Home with
  Games, Home-screen list, Blocks ready/play/drop/hold/stack/over/40
  lines/2 minutes/plain/1 preview, pause, pause after game over, New-game
  question, settings top and end, mode choice, How to play top and end;
  4096 new/moves/mid-slide/ramp/goal/over/5 × 5/6 × 6, pause, settings,
  How to play; Dark and Light, two in Amber). No existing golden changed.
* **`tests/settings/effects.toml`** did not exist: **created** with the 15
  game settings in the 07 §5.12 shape (id, name, scope, values, default,
  contexts, oracle, tests); `games::tests::settings_table` checks each game
  setting has an entry. VER-13 adds the rest and the matrix gate.
* **Gate**: host `cargo fmt --check`, `clippy --workspace --all-targets -D
  warnings` (also `orpheus-apps`/`orpheus-ui` without features), `cargo
  test --workspace`, golden tests: pass. Firmware: `fmt`, `clippy -D
  warnings`, `cargo xtask build` (resident 80,108 B), `size --check`
  (resident 80,107 B, bss 123,920 B: unchanged), `check-resident`, `stack
  --check` (21,812 of 22,016 B, margin 204 B: unchanged): pass. `cargo
  fuzz` (nightly-2026-11-05): `tiles_keys` 90 s (976 runs), `blocks_keys`
  120 s (1,692 runs), no finding; the bodies are slow (≈ 10–15 runs/s:
  every key checked against the references, each input run twice), a
  lighter fuzz-only mode is an open item. HIL: not run (no hardware in
  this package).
* **For the lead (firmware integration, after the stack-headroom
  package)**: enable `orpheus-apps/games` in `fw/firmware`; implement
  `GamesService` (a static `Hub` ≈ 848 B; requests answered by the card
  task: the three files under `/.orpheus/games/`; seed from uptime and the
  RTC; `display_on` from the power task), save the game on power-off and
  USB hand-over (`GamesRequest::Save`), `App::Games.on_player()` → true
  and the Games bit in the default Home mask (then the device suite's
  Home frames change), the settings store's key list grows by 15
  (`STORED` 113 ≤ 128). Not built: effects synthesiser and ducking,
  player names, Solitaire (GAME-2), artillery (GAME-4/5). User check
  (when on the player): play a game of each.

### 2026-11-25 — **soak `mixed` seed 1 passes on the player** (lead)

* Merged `a31ad6f3` (the 52–60 ms `job:index` was a root search after
  every index build: deleting a file emptied the whole walk cache; now it
  keeps it; the index lookup runs in resumable stretches of ≤ 3 reads) and
  added the `dir-walk` page cluster (11 lines: a 3-read stretch had taken
  33 ms of paging). Static stack 21,812 of 22,016 B.
* `cargo xtask dev soak mixed --minutes 30 --seed 1`: **PASS** — 35.6 min,
  27 actions (browse ×6 incl. Pictures and the library while playing,
  radio on/seek/keys, playdir, seek, set ×4, DSP, a USB personality switch,
  idle, long listens), 187 samples; stack free ≥ 8,528 B; 0 underruns
  outside disruption windows (3 inside, allowed); uptime drift −20 ppm;
  DAC +1,049 ppm against the workstation (budget ±2,500).
* Left for later (not failures): an `art` chore of 79 ms (unsplittable
  folder-art lookup, `chore: art 79/23 ms u79`) underran inside a
  disruption window — split it; evictions/s max 16,443 in one sample.
* Today's chain of soak findings, each with a regression test: USB-W1,
  stale test request, Shuffle-all watchdog, card loops never yielding,
  the log ring wipe, the check-resident hole, the headroom read race and
  uncounted underruns, paced refills, playback-path paging (AU-PERF),
  soft-I²C at a third of its speed, Up next titles (O(n) seek in a
  cluster), forced chores beyond the headroom, the index path lookup per
  piece, SD sleeps inside synchronous sessions, the chore margin, the walk
  cache wipe on delete.

### 2026-11-25 — AU-PERF: `job:index` in stretches; file removals keep the walk cache (worker, no hardware)

From the lead's soak (`chore: job:index 52/26 ms u52`, `pcm: head -621
ub=job:index:52ms`; earlier 59–60 ms): one unsplittable piece above every
budget (≤ ≈ 42 ms), so it was forced and underran once per first listing.

* **Where the 52–60 ms went** (host count, `orpheus-index`
  `tests/index_piece.rs`, `measure_index_piece`, `--ignored --nocapture`:
  owner-like images, exFAT 128 KiB clusters and FAT32 512-B clusters, a root
  of 300 or 1,000 long-named folders with `/.orpheus` made after them, 16
  shards of 40 or 100 index files, the walk cache as on the player —
  `/.orpheus/index` warmed and boosted, art and library walks after it — and
  the sector cache cold). r = sector reads, s = directory slots stepped.

  | part of the old piece | exFAT 128K, root 300 / 1,000, 40 per shard | FAT32 512, root 300 / 1,000 | 100 per shard (root 300) exFAT / FAT32 |
  |---|---|---|---|
  | shard search in `/.orpheus/index` (walk cache hit) | 1 r 6 s / 3 r 45 s | 1 r 15 s / 1 r 6 s | 1 r / 1 r |
  | shard + file search | 5 r 69 s / 7 r 108 s | 8 r 80 s / 8 r 71 s | 11 r 159 s / 13 r 170 s |
  | file open + header read (68 B) | 1 r | 1–2 r | 1 r / 2 r |
  | **whole piece, cache as on the player** | **6 r 69 s / 8 r 108 s** | **9 r / 9 r** | 12 r / 15 r |
  | **whole piece after an index build while idle** | **102 r 1,582 s / 324 r 5,121 s** | **108 r 1,587 s / 334 r 5,078 s** | 108 r / 113 r |
  | no index (shard searched to its end) | 9 r / 11 r | 12 r / 12 r | 20 r / 24 r |
  | next pieces: focus over 300 rows; 8 rows + letters | 20 r; 5 r | 24 r; 10 r | — |

  `resolve` is not on this path while playing (it is the idle `browse`'s),
  so there are no candidates or alt files to compare: the piece is the path
  walk to `/.orpheus/index/X/NAME`, the open and one header sector; the
  first rows are in the next sector (header 512 B, rows sector aligned), so
  header and rows never share one. **The time was the root search**: every
  index build (and library build) removes its scratch files at its end
  (`Builder` → `Storage::remove`), every `delete_entry` emptied the whole
  walk cache, so the mount-time warm-up of `/.orpheus/index` was gone after
  the first build and the next lookup searched the root. At 0.5 ms a read
  that is 56 ms for a 300-folder root (model), the soak's 52–60 ms; with the
  cache it is 5–14 reads (3–8 ms). Paging (≈ 0.3 ms a line) adds to either;
  not measurable on the host.
* **Fixes.**
  1. `orpheus-fs`: removing or renaming a **file** keeps the walk cache (a
     file is never cached; the entries around it stay put); a directory's
     delete/restore still empties it; FAT directory growth keeps it (a
     cached FAT directory is its first cluster and a fixed bound), exFAT
     growth still empties it. After a build the lookup is 5–14 reads again.
  2. `Volume::walk_step(&mut PathWalk, path, sets)`: the path walk in
     stretches of at most `sets` entry sets, resumable (`PathWalk`, 56 B,
     `Copy`; starts at the longest cached directory, caches the directories
     it passes, reports the found entry's `Locator`, again without I/O).
     `CardIo::find_step` (one stretch; the found place kept as `reuse`
     keeps one) and `CardIo::find_index(dir, spec, &mut walk, sets, left)`
     (stretches while `left()` says yes, then the open from the place and the
     header read: `Index::open`, the same checks), returning
     `Lookup::{More, Found(ix), Missing}`.
  3. Firmware: `Job::Counted` → `Job::Find { walk, sig, n }` (the lookup in
     stretches of `media::FIND_SETS` = 16 sets, `audio::piece_left` before
     each; state in the job, no new static) and `Job::Unsorted` (no current
     index: the folder's cursor for the build, asked for, else its own
     piece). Both are kind `job:index`; its seed 25 → 15 ms. `index_at`,
     the place rules and the validity rules are unchanged (the place comes
     from the found entry, checked by name and serial as before).
* **Per stretch** (16 sets): at most 3 reads / 48 steps on exFAT, 6 reads /
  84 steps on FAT32 512-B clusters (a FAT sector per cluster). In the host
  model (`chore::Piece` and `chore::Costs` as the firmware uses them, 20 ms
  budget, 0.5 ms a read + 3 µs a slot, cold sectors between pieces): cache
  as on the player one or two pieces of ≤ 5.5 ms; **cache lost** (root of
  300 / 1,000 searched) 8 / 18 pieces, the longest 9.2 / 14.7 ms, the
  longest stretch ≤ 3.7 ms; no piece over its budget.
* **Tests**: `orpheus-index` `tests/index_piece.rs`:
  `regress_index_lookup_card_operations` (owner-like exFAT 128K and FAT32,
  root 300, 40 per shard, a build before each of 3 listings: lookup ≤ 12
  reads / 200 steps, one stretch ≤ 4 (exFAT) / 6 (FAT32) reads, lookup +
  8 rows + letter table ≤ 30 reads; cache lost: a stretch ≤ 6 reads; fails
  with removals emptying the cache: 102 r 1,585 s),
  `index_lookup_pieces_fit_a_20ms_budget` (the model above; the old single
  piece is > 50 ms in it). `orpheus-fs` `tests/walk_step.rs`:
  `walk_step_on_every_format` (the 9-format matrix: same locator as
  `open_located` at stretches of 1, 2, 16, 1000 sets, case-insensitive,
  missing file/directory, `NotADirectory`, `..`, cached afterwards, a
  stretch of 2 sets ≤ 16 slots, fsck clean),
  `regress_removing_a_file_keeps_the_walk_cache` (FAT32, exFAT: scratch
  files made, renamed, removed: the lookup stays ≤ warm + 8 steps; fails
  with the old rule: 1,014 vs 5 steps; a removed directory still empties
  it, nothing stale served). The existing `walk_cache_on_every_format`
  (growth, renamed and removed directories) passes unchanged.
* **RAM / stack** (`xtask build` in this worktree, base `fe9a4c31`):
  resident 80,108 B unchanged, `__image_end` 0x1138ec and `__bss_end`
  0x131b00 unchanged, `__stack_bottom` 0x131d00 unchanged (usable 22,016 B;
  paged code 107 → 108 pages, slot 1,110,016 → 1,118,208 B;
  `Job` keeps its size: `Find` 84 B < `Rows`; `COSTS` unchanged, no new
  kind). Nested worst case 21,820 → **21,812 B, margin 196 → 204 B**
  (`Volume::walk` frame 568 → 560). The new chain below `io_session`:
  `CardIo::find_index` 320 B, 4,408 B in all (the `fill_some` chain it sits
  beside is 5,464 B).
* **Gate**: host fmt, clippy `-D warnings`, `cargo test --workspace`
  (1,130 passed, 8 ignored); fw fmt, clippy `-D warnings`, `xtask build`,
  `check-resident` (none paged), `size --check` (resident 80,107 B, bss
  123,920 B), `stack --check` (margin 204 B): pass. Not run: `xtask test all`
  beyond `cargo test --workspace`, brand/site checks, HIL (no hardware).
* **Lead, on the player**: the 30-minute `mixed` soak, seed 1. Expected:
  no `ub=job:index` and no `chore: job:index` line with `u` above a few ms
  (a stretch is ≤ 3 reads on the owner's card); the first listing after
  an idle index build no longer searches the root. If a `chore: job:index
  N/B ms uU` line still shows `U` ≥ 10 ms, the stretch is paging-bound
  (cold walk code), not card-bound: say so with the line. Also watch
  `job:focus` (a pass over ≤ 256 rows, 16–24 reads, one operation) — the
  next candidate if the first listing of a ≤ 256-row folder still
  underruns.

### 2026-11-25 — soak seed 1: one job:index underrun left; one stuck run (lead)

* Builds `3fd9…` (walk cache least-used eviction, 1/16 need decay) and the
  next (index folder warmed into the walk cache at mount; count/rows seeds
  20/16 ms). `job:count` overruns are gone. Remaining: **`job:index`
  52–60 ms**, one unsplittable piece above any budget (max ≈ 42 ms), so it
  is forced and underruns once per first listing (`ub=job:index:52ms`).
  The warmed walk cache did not remove it: the cost is not (only) the root
  search. Next (worker): split the index open into resumable steps.
* One soak run hung right after its start-up reboot: the player answered
  INFO, then not the next requests for over an hour; the log of that boot
  ends at "audio: state loaded" with **no "fw: healthy start"** (the
  health task's 30 s timer never completed). Not reproduced on the rerun.
  Open; recorded for the next occurrence (read `0x137FDC/E0` and the log
  before any reboot).
* Note: `pkill -f <pattern>` from the lead's shell matched its own command
  line again; use exact PIDs.

### 2026-11-25 — AU-PERF: the 55 ms `serve` block; precise `ub=` labels (worker, no hardware)

From the lead's soak (`mixed` seed 1, `pcm: head -817 … dec=6ms mk=0
ub=serve:55ms`, 2 underruns, `tests/regress/soak/mixed-underrun-seed1-4.soak`).

* **Every path of the playback loop that runs while the DAC plays**
  (`play_file` → `play_block`), and whether it can block it for tens of ms:

  | path | rule | noted as (before → now) | tens of ms? |
  |---|---|---|---|
  | `chores`: served request (`serve_one` → `Server::serve`), listing piece (`step`), read-ahead open/head, library place, art | `chore::decide` | `serve`/`pf`/`prep`/`lib`/`art` → `serve:<req>`, `job:<piece>`, `pf`, … | yes, only as an **overrun** of its budget (≤ 42 ms): logged `chore:` — but at most one line a second |
  | battery / step log writes (`drain_flush_rows`, `sensor_flush_rows`) | `log_due` (decide, roomy, never forced) | `serve` → `drain` / `sensor` | yes (FAT append or truncate + flush; SD write busy) when the logger runs, the steps file is due or a trace is on; overrun logged |
  | player requests in `head` (`play_req`, `edit`: skip, pause, shuffle, A–B, bookmark, Up next edits) | none (CPU only, no card) | **not noted** → `req:<label>` | no (the player's code may page in: a few ms) |
  | skip / stop / new queue / **radio start** / card hand-over / power-off | — | the file ends: `out.stop()` aborts the DMA, no look, **no underrun counted** | n/a (abrupt cut, no ramp) |
  | seek | — | `out.stop()` first, then `seek_to` reads the card | n/a (DAC stopped) |
  | refill | — | `fill` | read-ahead's head only when roomy |
  | gapless join (next decoder's open with the DAC running) | — | `join` | yes (Vorbis setup): its own label |
  | `after_block` / `reader_check` player events (`Tick` once a second, `ReaderAtEnd`, position), a Sound change (`apply_sound`) | none | not noted | no (sub-ms) |
  | **a chore while the DAC is stopped** (track start, after a seek or a resume: `headroom()` is `i32::MAX`, budget unlimited) | decide: runs anything | noted; never logged (no budget to overrun); **the candidate stayed in the ledger** until the first clean half after the start | n/a (no deadline), but a later underrun could be charged to it |
  | `serve_one` elsewhere | — | only from `chores`; `head`'s `Some(Err(req))` → `srv.serve` only from the **paused loop** | n/a (DAC stopped) |
  | paused loop (`pause`: state save, logs, listing pieces with no budget) | none | not noted | DAC stopped: no underrun; a resume waits for the piece running |
  | starved chore | decide `Starved` (roomy) | as its kind | logged `chore: starved` |
  | other tasks (UI, USB, Bluetooth, radio on the thread executor) | — | — | cannot preempt the RT audio task, **except a NOR erase/program** (BASEPRI-masked; 4 KiB erase typ. ≈ 45 ms): the stall lands inside whatever operation runs |

* **Radio start**: opening Radio (`RadioScreen::enter`) → `Fw::radio_request`
  → `PlayReq::Stop` → `head` → `End::Idle` → `out.stop()`. A deliberate,
  clean stop: nothing is counted, so the two underruns were **not** caused
  by it, and the soak was right to count them. But it could not say where
  they came from: the read in `radio-keys` covered 28 s since the last one
  in `browse pictures` (the track went 27,984 → 54,504 ms: it played to
  ~1.5 s before that read, i.e. to the Radio entry). The underruns are from
  the end of `browse pictures` or the navigation Pictures → Home → Radio,
  with music playing.
* **What the 55 ms was** (no hardware here; ranked): (1) **a chore that
  overran its budget by ≥ 13 ms whose `chore:` line was swallowed by the
  one-line-a-second limit** (`job:rows 16/16`-style lines are routine during
  a listing): every `serve`-noted operation is inside `piece_begin`/
  `piece_end` with a budget ≤ 42 ms while the DAC runs, so a 55-ms one *was*
  an overrun; likeliest kinds: one whose need had decayed on a variant that
  reads no card (`want` while a folder plays: see fix 4), `serve:browse`'s
  cursor or `job:index` on a new folder, or a NOR write landing inside a
  chore; (2) a stale candidate from an unlimited chore before a DAC
  (re)start (no restart in that window: low); (3) a step-file write
  (`sensor`; needs steps walked). The next run names it.
* **Fixes**:
  1. **Ledger labels** (`orpheus_audio::Ledger`): a detail byte with the
     candidate (`note_as`); the `pcm:` line names the exact path,
     **first after the headroom** (the 96-byte line cut long ones):
     `pcm: head -817 ub=serve:picture:55ms blk w=… mk=0 +0`. Labels: chore
     kinds (`pf`, `prep`, `lib`, `art`, `drain`, `sensor`,
     `job:count|index|focus|rows|win|qtitle`, `serve:<request>`), player
     requests `req:pause|skip|seek|stop|shuffle|ab|mark|edit|other`, else
     the cause (`fill`, `join`, `dec`). Counts (`status ub:`) unchanged (8
     causes; `req:`, `drain`, `sensor` count under `serve`). Ledger
     44 → 24 B (u16 counts, saturating; no `Option`, all zeros: `.bss`).
  2. **The DAC's start forgets the candidate** (`PcmOut::start` →
     `kept_up`): work done while it was stopped cannot be charged.
  3. **Large overruns always logged**: a piece ≥ `LOUD_OVER_US` (4.4 ms,
     a fifth of the margin) over its budget, or starved, bypasses the
     one-line-a-second limit.
  4. **A piece without card work does not lower its kind's need**
     (`Costs::observe_piece`, `Piece::with_io`/`worked` over
     `yoton_bsp::card::io_count`): `want` while a folder plays only starts a
     listing job, so five answers from RAM took a 25-ms `want` need to
     ~6 ms and the next library-list window ran at a budget of a few ms.
     Raises still count.
  5. **Soak**: every action is read at its end (`Ctx::boundary`, if the last
     read is ≥ 2 s old), and the Radio entry is read just before the OK
     that stops the music, after which its disruption window opens. A
     finding now names the action it happened in; the clean radio stop
     gets the source-change allowance like `radio-on`.
* **Not done**: a ramp on deliberate stops (skip, stop, radio): the DMA is
  aborted mid-half (no underrun, possibly a click). Splitting `want` into
  folder/library kinds (fix 4 covers it without new statics).
* **Tests**: `orpheus-audio` `ledger::tests::regress_the_candidate_keeps_its_detail`;
  `chore::tests::regress_a_piece_without_card_work_does_not_lower_the_need`
  (host model, the soak's load, 1 kind of 25 ms with 5 RAM-only answers in
  6: 0 underruns on 5 seeds; measuring every piece as before: ≥ 1 underrun
  on every seed, 6 in all); `xtask` `dev::soak::tests::regress_underruns_are_read_in_the_action_they_happen_in`
  (simulator; fails with the boundary read disabled).
* **RAM / stack**: bss 123,936 → 123,920 B (−16: the ledger), resident
  80,091 → 80,107 B (+16: linker-merged constants; the new label strings
  are paged statics, as plain literals they cost +32), so
  `__stack_bottom` stays at 0x131D00: usable 22,016 B, nested worst case
  21,804 B, margin 212 B (unchanged; still exactly on a 256-B step).
  `play_block` / `chores` frames unchanged on the worst chain.
* **Gate**: host `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo test --workspace` (1,123 passed) pass;
  fw fmt, clippy `-D warnings`, `xtask build` (release), `check-resident`
  (none paged), `size --check`, `stack --check` (margin 212 B) pass. Not
  run: `xtask test all` beyond `cargo test --workspace`, brand/site checks,
  HIL (no hardware).
* **Lead, on the player**: (1) `cargo xtask dev soak --replay
  tests/regress/soak/mixed-underrun-seed1-4.soak`: if it still underruns,
  the finding names the action and the `pcm:` line the path; a 55-ms
  overrun now has its own `chore:` line. (2) `dev soak mixed --minutes 30
  --seed 1`: collect every `pcm: head` line (`ub=<path>`) and `chore:`
  line. `ub=serve:<req>` / `job:<piece>` with a matching `chore:` line:
  that kind needs splitting or a higher seed; `ub=req:<label>`: a player
  request (paging); `ub=drain|sensor`: the log write; a `serve:`/`job:`
  label with **no** `chore:` line: something outside the chore (a NOR write
  inside it) — then say so.

### 2026-11-25 — chore margin calibrated; one uncontrolled 55 ms block left (lead)

* Host model calibrated to the player (decoder 4–9 ms, +5 ms code reload
  after a chore, refills 3–8 ms, listing operations up to 21 ms); it
  reproduced the soak's underrun at the old 10 ms margin
  (`regress_margin_covers_the_reload_after_a_chore`); `MARGIN_US` now
  22 ms (no change for the heavy-load cases, whose margin follows the
  decoder peak).
* 30-min seed 1 on that build: no reset; loads/s 1,044, idle 71 %, stack
  ≥ 9,360 B; chore pieces now keep to their budgets (`job:rows 16/16`,
  `job:count 25/25`), but **2 underruns from one 55 ms block labelled
  `serve` that no `chore:` line accounts for** — work that bypasses
  `chore::decide` (around `browse pictures` → `radio-keys`). Next: find
  that path (worker).

### 2026-11-25 — fix: SD waits slept inside synchronous sessions (watchdog reset) (lead)

* The first soak on `13b0ce70` (index places) lost the player at the start:
  a watchdog reset, last interrupt sample in `atj2157_hal::sd::Sd::wait_ts`.
  A transfer that outlasts the driver's short spin sleeps on an Embassy
  timer; inside a synchronous card session (`orpheus_index::card::spin`,
  used more now that listing pieces read through one session) that timer
  never fires — the same trap the card pacing hit.
* Fix: `Sd` waits are a `nap!` macro — a timer sleep normally, a spin on
  the timer counter (`delay::wait_us`, bounded) when the card driver marks
  the call as inside a synchronous poller (`Sd::set_no_sleep`, from
  `orpheus_fs::SyncPoll::active()`; the flag shares a byte with the
  high-capacity bit). On-device regression: the 30-min soak (seed 1).
* Stack: a first version cost two 256 B steps (a static, then larger task
  futures through an extra `async fn`) and moved `spin` out of `spin_in`,
  copying the file-open future twice (+1.5 KB on the RT chain); now
  `spin_pinned` polls in place. Result: 21,804 of 22,016 B, margin 212 B.
  **`__stack_bottom` is exactly on a 256 B step (0x131D00): any further
  static byte costs 256 B of stack** — the next feature needs a stack
  reduction first.

### 2026-11-25 — AU-PERF: listing pieces reopen the index file from its entry (worker, no hardware)

From the lead's soak (`chore: job:rows 37/36 ms u31`, 11 underruns): each
piece opened a new session and found `/.orpheus/index/X/NAME` by path.

* **The 31 ms was that path walk** (host count, `orpheus-index`
  `tests/rows_piece.rs`, `measure_rows_piece`: a firmware-style piece on
  FAT32 (512-B clusters) and exFAT (32 KiB) images with 50–1,000 long-named
  folders in the root, `/.orpheus` made after them, 4–60 index files per
  shard, the sector cache cold). The piece's first stretch (session open +
  first rows read, before its first ask) by path: with the walk cache
  holding `/.orpheus` 0–19 sector reads (15–206 directory steps; the shard
  is searched), with it lost 22–24 reads for a 50-folder root, 132–150 for
  400, 333–342 for 1,000 (2,042–5,202 steps). One row after it: 1–2 reads.
  At 0.3–1 ms a read, 31 ms is a root search of a root of roughly
  100–250 entries, or the shard plus pager loads of the walk code; nothing
  else in that stretch is longer than one data sector. The last stretch of
  a finished window had a second walk (`load_letters`' own session). The
  three-slot walk cache loses `/.orpheus` to the art and library walks
  (shown in the measurement).
* **Places** (`orpheus_index::card::Place`, 12 B: the file's `Locator` +
  the volume serial). A read-only `CardIo` records where it found a file
  (`place`) and reopens it from a given place (`reuse`); new
  `orpheus_fs::Volume::open_located` (walk, read-only, returns the
  locator) and `Volume::open_at(loc, name)` (the entry read again: seek +
  its sectors; refused unless a set starts there and has that name; a
  file rewritten in place opens with its new clusters and size). Another
  serial, a gone or renamed entry, or another file in the slot: by path
  again. From a place the first operation costs 2–5 reads and 3–14 steps
  on every image above, walk cache lost or not.
* **Firmware.** `Server::index_at` (16 B, set and dropped with `index`):
  `job:index` (the listing's one walk) keeps the place; `job:focus`,
  `job:rows`, the window while idle (`Want`), the queue from the index use
  it (`media::with_index`). `job:rows` is one session: `fill_some` asks
  `piece_left` before the rows read too (it opens the file), and the letter
  table is read in it when the budget lasts, else in the next piece (the
  window is published `complete` after it). The library's read sessions
  (`library::with_lib`: window, list, queue, sample, the gapless lookup
  steps) keep the library file's place in the decoder arena after the
  build slot (`library::STATE` +16 B, no static).
* **The need counts the fixed cost** (`orpheus_audio::chore::Piece`): the
  first ask always says yes, so the start → second ask is one gap. The
  soak's piece (31 ms before the first ask + 5 ms row) now measures 36 ms,
  not 31, so a 36-ms need is not admitted below a 36-ms budget. Pieces that
  ask first (count, window, Up next, the model) measure as before.
* **Tests**: `regress_later_rows_pieces_do_no_path_lookup` (400-folder
  root, 60 files per shard, 5 later pieces with the art and library walks
  and a cold sector cache between them, the walk cache dropped every other
  piece: first operation ≤ 5 reads / 16 steps FAT32, ≤ 3 / 4 exFAT; fails
  with places ignored: 19 / 206); `a_kept_place_is_revalidated` (another
  serial: by path, the new place that card's; the index rebuilt in place
  with 40 more rows: opened from the place with its new header and rows;
  removed and its slot taken by another index file: not read, no index, no
  place); `orpheus-fs` `tests/open_at.rs` (every format: case-insensitive
  name, another name, directory, rewritten longer, renamed, removed,
  slot reused; fsck clean);
  `regress_work_before_the_first_ask_is_one_unit_with_the_first_operation`.
* **RAM / stack** (`xtask build` in this worktree, base `d1c548eb`):
  `.bss` 123,920 B unchanged, stack start (`xtask stack`) `0x131ce0` →
  `0x131cf0` (+16 B, `Server::index_at`; **16 B left before the next 256-B
  step**), usable stack 22,016 B unchanged. Nested worst case 21,740 →
  21,772 B (margin 276 → 244 B): the deepest chain is now the listing
  piece (`Server::step` 720 → 744 B, `io_session` 328 → 408 B: the
  session's kept place, 36 B, and the closure), 13,728 B in RT, above
  the Vorbis seek chain (13,696 B). Resident 80,076 → 80,092 B.
* Gate: host fmt, clippy `-D warnings`, `cargo test --workspace`; fw fmt,
  clippy, `xtask build`, `check-resident`, `size --check`, `stack --check`:
  pass. Not run: anything on the player.
* **Lead**: the 30-minute `mixed` soak, seed 1. Expected: no `job:rows`
  line with `u` ≥ 10 ms after a listing's first piece; `job:rows` pieces
  within their budget; the underruns from `serve:` gone (the 11 were all
  `ub=serve:40ms`). `job:index` (the one walk per listing, own need) may
  now be the longest `job:` line: its `u` is the root search on this card.
  If it overruns, the next lever is keeping `/.orpheus/index` out of the
  walk cache's round robin (or a place per shard).

### 2026-11-25 — chore budgets on the player (lead)

* Merged `0d2e672b` (chore budgets, `orpheus_audio::chore`); all gates
  pass, static stack unchanged (21,740 / 22,016 B).
* Replay `mixed-underrun-seed1-6.soak`: 1 underrun of 37 frames
  (`chore: job:count 19/18 ms u16`). Raising the margin by a fixed
  code-reload allowance (8 or 4 ms) made the host model starve 30 ms
  chores; reverted (the model's decoder, 14–19 ms per call, is twice the
  player's 4–9 ms).
* 30-min seed 1: CPU idle 71 %, loads/s 1,058, stack free ≥ 7,744 B; 11
  underruns at 7.7 min from **`job:rows` pieces of 37–40 ms against
  36–38 ms budgets, with a single card operation of 31 ms (`u31`)**. Each
  piece opens a new index I/O session (files reopened by path) whose cost
  is not in the piece's estimate. Next (worker): keep the index handles
  across pieces, count the session overhead in the need, find the 31 ms
  operation.

### 2026-11-25 — AU-PERF: chore budgets (no chore may empty the DAC ring) (worker, no hardware)

Problem (lead, soak seed 1 on `942c258c`): under UI load the headroom
seldom reached 1.5 halves, so after 300 ms of waiting a chore ran at *any*
headroom; chores of 25–55 ms then ran at −93…−649 frames (1 underrun each).

* **The rule** (`orpheus_audio::chore::decide`, a pure function of the
  headroom, the DAC rate, the chore's measured need, its wait and the
  recent decoder calls). Budget = headroom in µs − margin, margin =
  max(10 ms, 1.25 × the longest recent decoder call, `DecPeak`: decays
  1/256 per call). The first chore due (fixed order: read-ahead open,
  read-ahead head, request, listing piece, library/art; the others wait
  behind it) runs **only if budget ≥ its need**, and then when roomy
  (≥ 1.5 halves) or after 300 ms of waiting. A need that no headroom
  covered for 2 s (longer than the ring) runs at the next roomy moment
  anyway (`Starved`, logged): the UI always gets an answer. The battery
  and step logs: roomy, need covered, no chore waiting, never forced.
* **Pieces** (`chore::Piece`, firmware `audio::piece_left`): splittable
  chores ask before each card operation; the first always goes, later ones
  only if time spent + the longest operation known fits the budget; they
  resume in the next chore. **Needs** (`chore::Costs`, 27 kinds, u16 in
  16-µs units, `.bss`): the longest unsplittable stretch per piece (gap
  between two asks, or the whole piece), rising at once, decaying ¼ of the
  way per shorter piece; seeds (µs) until measured: pf 12k, prep 10k, lib
  6k, art 15k, drain/sensor rows 20k; job count 8k, index 25k, focus 15k,
  rows 8k, win 8k, qtitle 8k; requests browse 10k, want 15k, text 15k,
  queue-win 2k, picture 30k, lib-list 20k, drain/sensor 15k, otp 25k,
  delete/update 30k, release/image/rebuild/alert 1k.
* **Made splittable**: the listing count (per entry), index rows (per row;
  was per 4), **the window of an unsorted folder while playing** (new
  `Job::Win`: was the whole `Server::window` in one request), Up next titles
  (several per piece while the budget lasts), the library search (steps
  while the budget lasts), the read-ahead's head (`prepare`: one 2-KiB read
  per operation, resumed across chores). `PIECE_US` (fixed 8 ms) is gone.
* **Not split** (run only when the headroom covers the measured need):
  the read-ahead open (`open_target`), Now Playing art from the cache
  (`serve:picture`: bucket path open + 52² or 128² px, up to 64 sector
  reads — the likeliest long one), `serve:text`, the library window /
  list (`publish_list`), `serve:browse`'s cursor, `job:index` (the index
  file's path open; with no index also `want_build`), `job:focus`
  (`position_of` over ≤ 256 rows), the gapless track's art, the log rows,
  and one card operation of every splittable chore. Each has its own need,
  so the measured worst case decides.
* **Log**: `chore: <kind> took/budget ms u<longest stretch> +<not logged>`
  when a piece overran its budget, `chore: starved …` for a starved run;
  at most one a second. Kinds: `pf`, `prep`, `lib`, `art`, `drain`,
  `sensor`, `job:count|index|focus|rows|win|qtitle`, `serve:<request>`.
  `pcm:`, `serve:` (≥ 30 ms) and `qtitle` unchanged. A request is looked
  at with `REQS.try_peek` (its kind decides) and taken when it runs.
* **Host model** (`chore.rs` tests): DAC ring of 2 × 1,536 frames at
  48 kHz, MP3 blocks, decoder calls and refills as load, a UI that asks
  one thing at a time, a read-ahead open every 5 s, the costs learning.
  `regress_forced_chore_never_exceeds_headroom` (soak load: calls 14–19 ms
  per 24-ms block, refills 3–8 ms every other block, UI 5–60 ms think
  time; 5 seeds × 120 s): 0 underruns, 0 starved, the forced path taken,
  worst answer < 2.5 s; `old_rule_underruns_under_the_soak_load`: the old
  rule ≥ 1 underrun per seed, ≥ 10 over the 5 (asserted) on that load;
  `every_chore_that_fits_is_answered_in_bounded_time` (4 loads from light
  to the soak's: worst answer < 0.6 / 0.8 / 1.5 / 2.5 s, no underrun; at
  ≈ 85 % load the 28-ms request is answered by the starvation escape,
  still no underrun, < 5 s); `a_chore_that_cannot_fit_is_still_answered`
  (80 ms unsplittable: answered within 2.2 s, one underrun per piece, the
  only case that can still run the ring dry — the `chore: starved` line
  names it); unit tests of the budget, rule, piece and costs.
* **Third DAC half: not done.** The model closes the gap without it.
  Trade-off: +1,536 frames (+32 ms at 48 kHz, +6 KiB). SRAM3 is full (rodata
  page cache 16 KiB, PCM ring 12 KiB, profiler 2 KiB, shared 2 KiB), so it
  would come out of the rodata cache (16 → 10 KiB: more rodata faults) or
  main RAM (the stack: not possible). Worth it only if a kind that cannot
  be split shows `chore: starved` on the player.
* **RAM / stack**: resident 80,068 → 80,076 B, bss 123,808 → 123,920 B
  (+112: needs 56, piece 28, log counters 8, read-ahead state, futures);
  stack region bottom 0x131C68 → 0x131CE0, usable unchanged at 22,016 B
  (usable goes in 256-B steps: **the next step down is at 0x131D00, 32 B
  away**). Nested worst case 21,740 B unchanged, margin 276 B. Frames:
  `chores` poll 232 → 200 B, `play_block` 208, `step` 720 (unchanged;
  titles and the unsorted window poll through `Own`), `prepare` its own
  `Apart` frame (328), `chore_due` 80. `play-block` cluster: + `chore::
  decide`, `need_of` (run every block while a chore waits): 15 lines
  (15,052 B), the 32-line fit holds. `stack.toml`: `chore_due` calls the
  waker (`try_peek` wakes a sender of a full channel).
* **Gate**: host fmt, clippy `-D warnings`, `cargo test --workspace`
  pass; fw fmt, clippy `-D warnings`, `xtask build`, `check-resident` (none
  paged), `size --check`, `stack --check` (margin 276 B) pass. Not run:
  `xtask test all` beyond `cargo test --workspace`, brand/site checks, HIL.
* **Lead, on the player**: (1) `cargo xtask dev soak --replay
  tests/regress/soak/mixed-underrun-seed1-6.soak`: expect no finding and
  no `pcm: head` line. (2) `dev soak mixed --minutes 30 --seed 1`: 0
  underruns expected; collect the `pcm:` lines (any `ub=` names the cause),
  `serve:` lines (≥ 30 ms requests, now only at ≥ 40 ms headroom), and
  every `chore:` line: an overrun names the kind whose need was too low
  (the first piece of a kind, or a longer operation than seen before), a
  `starved` line a kind that fits no headroom (split it, or the third
  half). (3) Browse Files/Pictures with a shuffled queue: answers should
  feel as before; slow ones show as `chore: starved`.

### 2026-11-25 — seed-1 soak after the performance work (lead)

* `dev soak mixed --minutes 30 --seed 1` on `942c258c`: ran 21.4 min;
  **line loads/s mean 1,076 (was 1,700–2,500), CPU idle mean 72 % (was
  29–51 %), stack free ≥ 9,360 B (was 5,300–6,400), uptime drift +5 ppm**.
  Stopped on **1 underrun** (`serve`, headroom −93 frames) during
  `browse pictures` with a 256-track shuffled queue
  (`tests/regress/soak/mixed-underrun-seed1-6.soak`); the `pcm:` lines show
  chores of 25–55 ms (was 300–465 ms).
* Cause: `chore_due` runs chores when the headroom is ≥ 1.5 halves
  (48 ms) **or after 300 ms of waiting at any headroom**; under UI load the
  headroom seldom reaches 1.5 halves, so a 25–55 ms chore is forced in at
  low headroom. Open: cap every chore piece at ≤ 8 ms of card/CPU work
  (the listing job already has `PIECE_US`; the queue title and request
  serving do not), or a third DAC half (+6 KB RAM).

### 2026-11-25 — Up next titles cheap; Pictures-while-playing replay passes (lead)

* Merged `cb99dec0` (`seek_dir` jumps inside a cluster: 3,507 → 7 steps
  per title on 128 KiB exFAT clusters; stale locators fail in ≤ 2 reads;
  Up next keeps titles across window moves). All gates pass; static stack
  21,740 of 22,016 B.
* On the player: `soak --replay tests/regress/soak/mixed-ring-seed1-7.soak`
  → **7 actions, no finding** (was 20 underruns); no `pcm: head` lines on
  that boot. `fuzz-ui --replay tests/regress/ui-device/underrun-seed8-685.keys`
  (hold Back with a shuffled queue: 18 `serve` underruns on the previous
  build) → no finding.
* The cover search (before playback starts) still reads ~37 entries per
  250 ms, ~1,000 line loads: paging-bound; capped, does not affect playback.

### 2026-11-25 — stack: static analysis merged, worst case 21,740 of 22,016 B (lead)

* Merged VER-10 `f6fcc23e` (`cargo xtask stack`: static worst-case depth
  per context with nesting, indirect calls resolved through `stack.toml`;
  cuts in the UI start-up, HMAC, index/media frames, Vorbis, a
  non-recursive panic handler). Together with AU-PERF's smaller audio
  frames: nested run-phase worst case **21,740 B of 22,016 B usable**
  (276 B margin; RT 13,696 B through idle → `play_file` → `seek_to` →
  Vorbis setup; thread 6,876 B). Budget ratcheted to margin ≥ 0; the goal
  remains ≥ 4 KiB (next: the audio task poll 2,328 B, `idle` 2,032 B,
  `play_file` 2,320 B frames).
* On the player: `fuzz-ui --replay tests/regress/ui-device/stack-seed8-340.keys`
  → **no finding** (was stack free 1,912 B). Full 30-min settings fuzz
  rerunning.
### 2026-11-25 — AU-PERF: Up next titles cheap (`open_locator` seeks per cluster) (worker, no hardware)

* **Host measurement** (`crates/orpheus-fs/tests/locator_cost.rs`,
  `measure_open_locator`, `--ignored --nocapture`). One `open_locator`
  with a cold 8-slot cache (as after the playback loop's reads), by entry
  position, folder size (200/2,000 files), name length (11 vs 60 chars)
  and layout (contiguous; fragmented: the folder's clusters interleaved
  with another folder's and file data, so its chain goes through the FAT).
  "Steps": directory positions stepped one at a time (new host-only
  counter `Volume::dir_steps`, not in the firmware build).

  | format | case | slot of entry | steps before → after | cold reads before → after |
  |---|---|---|---|---|
  | exFAT 128 KiB clusters (the owner's card) | 2,000 long names | 3,500 | **3,507 → 7** | 2 → 2 |
  | exFAT 128 KiB | 2,000 long names | 7,000 / 13,993 | 2,912 / 1,715 → 8 / 10 | 1 → 1 |
  | exFAT 128 KiB | 200 long names | 1,393 | 1,400 → 7 | 1 → 1 |
  | exFAT 32 KiB | 2,000 long names | 7,000 | 869 → 13 | 1–2 → 1–2 |
  | FAT32 4 KiB clusters | 2,000 long names | 11,996 | 191 → 99 | 3 → 3 (6 fragmented) |
  | FAT32 4 KiB | 2,000 short names | 2,001 | 97 → 16 | 2 → 2 |

  Sector reads were never the problem: 1–3 per call on exFAT, at most 6
  on fragmented FAT32 with small clusters (one FAT sector per hop), 0
  warm. The cost was the seek: inside the entry's cluster it stepped slot
  by slot (`advance` per 32-byte slot), up to 4,095 steps in a 128 KiB
  cluster: host time grew with the slot (13.6 µs at slot 3,500, ~57
  instructions per step), on the player ~3,500 × (1–2 µs, or a code-line
  load each if the seek loop and `advance` share a physical line of the
  pager's direct-mapped cache) — consistent with the measured 70–120 ms.
  Host time after: ≤ 1.1 µs for every case.
* **`seek_dir`** jumps inside a cluster (or the FAT12/16 fixed root) in one
  step: one step per cluster crossed. **`open_locator`** looks at the slot
  first and fails at once when no entry set starts there (a stale locator:
  the entry deleted, or the folder changed); `read_dir` used to scan on to
  the next set: measured 127 reads and 1,800 steps for a locator before a
  run of 299 deleted files, now ≤ 2 reads. Validity rules unchanged (the
  entry at the index, with that index, or `NotFound`).
* **Up next window keeps its titles** (`Server::keep_titles`,
  `orpheus_player::queue::shift_window`/`window_prefix`, host-tested):
  when the window moves (a track ended: by one) the titles it had for the
  same queue generation move to their slots and are shown at once; only
  the missing ones are looked up (one per chore while playing, the first
  missing slot first; idle all at once, published once). The queue
  generation now also changes when the card goes to USB and after a
  delete (`Server::titles_stale`), so no kept title outlives a change on
  the card. While playing a window move costs one lookup instead of 8.
* **`qtitle ms/io/ld A/B/C`** log line when one title takes ≥ 20 ms: ms,
  card operations, pager line loads — splits card time from paging.
* **Not done** (not needed by the measurement): titles from the library or
  a title file under `/.orpheus/`, a cluster cache for FAT-chained
  folders. If the player still shows slow titles, the log line says which.
* **RAM**: no static added (bss 123,808 B unchanged); resident 80,100 →
  80,116 B (+16 B, the log strings), so `__stack_bottom` 0x131C88 →
  0x131C98 (**−16 B of stack**). Frames: `Server::queue_title`'s poll is a
  frame of its own now (124 B, below `step`; it was inlined),
  `keep_titles` 20 B, `audio::hand_over` 300 → 308 B; `open_locator`,
  `fill_rows`, `fill_some` unchanged.
* **Tests**: `regress_open_locator_steps_per_cluster` (FAT32 4 KiB,
  exFAT 128 KiB, 600 long names: every entry ≤ clusters crossed + 22
  steps and ≤ 3 cold reads, name checked; the old seek took up to
  ~3,600 steps there), `regress_stale_locator_fails_fast` (FAT32, exFAT: ≤ 2
  reads; fails without the check: 127 reads), `seek_dir_matches_sequential_listing`
  (all 9 formats of the matrix, root incl. fixed FAT12/16 root and a
  multi-cluster folder: forward, back, past the end), orpheus-player
  `shift_window_keeps_the_shared_positions`.
* **Gate**: host fmt, clippy `-D warnings`, `cargo test --workspace` pass;
  fw fmt, clippy pass; `xtask build --release`, `check-resident` (none
  paged), `size --check` pass. Not run: `xtask test all` beyond
  `cargo test --workspace`, brand/site checks, HIL (no hardware).
* **Lead, on the player**: (1) `soak --replay
  tests/regress/soak/mixed-ring-seed1-7.soak`: expect 0 `serve` underruns
  from Up next (was 20). (2) Now Playing › Up next on the Shuffle-all
  queue while playing: `serve:` lines should no longer name `queue-win`;
  no `qtitle` line expected (each < 20 ms); if one appears, its io/ld
  numbers say card or paging. (3) `pcm:`/`status` underruns stay 0 across
  track changes with Up next shown (one lookup per change).

### 2026-11-25 — AU-PERF on the player; Up next window found blocking playback (lead)

* Merged AU-PERF (`0a02b8b6`): per-block path out of the async closures,
  `play-block` 77 → 15 lines, clusters laid out for the direct-mapped
  cache, `Chain::active` cached, every decoder's `decode` out of line, soft
  I²C timed on T0. On the player: `steps status` bus 1.65 ms per sample
  (`bus_load` 4.1 %, was ~7 %), `acc read` 1,000 mg, 0 I²C errors; the
  cover search reads ~100 entries per 250 ms (was ~30).
* Pictures replay still underran, cause `serve`: new log line names slow
  requests — **`queue-win` 465 ms** (Now Playing's Up next window: each
  title opens a track of the 256-track shuffled queue by locator). Now
  filled one title per chore while playing (`Server::queue_title`); each
  piece still takes 70–120 ms (`open_locator` scans the folder up to the
  entry), more than the ~64 ms of DAC ring: 20 underruns remain on the
  replay. Next: make titles cheap (library titles, a title cache, a
  faster locator seek) — worker.
* Line loads/s during the step stayed ~3,200/s (the step is dominated by
  card-task work, not the playback loop now).
### 2026-11-25 — VER-10: static worst-case stack depth (worker, no hardware)

Trigger: VER-8 measured 1,912 B free (budget 2,048 B) after
`tests/regress/ui-device/stack-seed8-340.keys`.

**Tool** (`cargo xtask stack`, 07 §4.4 "Static worst-case stack depth"):
`linkgraph` now keeps the direct branches between code sections (`BL` =
call, `B` = tail call); frames come from `metrics::parse_functions`
(running SP sum over the whole function, frame-pointer epilogues tracked;
the DWARF CFI follows `r7` and does not give SP); contexts, indirect
targets and bounded recursion in `stack.toml`; nesting per phase (`init`:
`main` before the executors; `run`) with 108 B per exception (eager FPU
frame + alignment). Gate step `stack --check` (07 §6); `xtask metrics`
records `stack_static` rows. Host tests: synthetic graphs (tail calls,
indirect sites resolved / bounded / unresolved, recursion, nesting),
objdump listing (shrink-wrapped and duplicated prologues, `sub sp, r7`,
self-calls), `stack.toml` parses.

**Stack**: 22,296 B region (`0x131ce8..0x137400`), 22,016 B usable above
the 256-B-aligned MPU guard (21,760 B before: `.bss` shrank 48 B and the
guard alignment moved; the usable size varies by up to 255 B with `.bss`).
The VER-8 measurement (1,912 B free of 21,760 B) means **19,848 B used**.

**Static worst case per context** (before → after, bytes):

| Context | Prio | Before | After | Deepest root / chain after |
|---|---|---|---|---|
| init (`main` before the executors) | thread | 12,368 | 12,368 | `audio::init_chain` 11,192 B frame (the DSP chain built on the stack); RT and USB do not exist yet |
| thread (main executor) | thread | ≥ 10,736 | 6,876 | UI task 6,772: `Fw::derive` → `derive_finish` 4,008 (the vault's finish future) → seal → SHA-256. Before: `new_ui` 4,824 (the whole `Ui` temporary) → root `enter` → Authenticator → HOTP/SHA-512 |
| irq-normal (USB) | L3 | 80 | 80 | `usb::on_irq` |
| rt (RT executor) | L2 | ≥ 18,040 | 14,856 | audio task closure **5,840** → `play_file` **2,296** → `seek_to` **1,488** → `VorbisDecoder::step` 808 → `load_setup` 1,320 → `setup::setup` 2,576 → sort → panic. Before: … `play_file` → `Server::serve` 2,352 → `resolve` → index build → `CardStore::write_at` → `Inner::slot` 2,560 → `Volume::open` … → SD → time driver → panic (≥, the panic handler recursed) |
| deadline (time, DMA) | L1 | ≥ 600 | 312 | `time::on_t1` (its panic tail shrank) |
| pager | L0 | 236 | 236 | `orpheus_memmanage` → `orpheus_pager_fault` → `PagerCore::touch` |
| prof (T2 sampler) | L0 | 80 | 80 | |
| fault (own 512 B stack) | — | 32 | 32 | `fault_entry` → `record_fault` |
| **nested, run** | | **≥ 30,232** | **22,900** | thread 6,876 + L3 80+108 + L2 14,856+108 + L1 312+108 + L0 236+108 + fault frame 108 |
| nested, init | | ≥ 13,528 | 13,240 | |

Other task roots after: USB task 3,424 (DevChannel `control_out` → OTP
URI parse), Bluetooth 1,424, radio 816, steps 688, power 520, watchdog
472, input 424, health 416, `rt_beat` 368. Indirect calls: all resolved
(task polls, `irq_dispatch` handlers, waker vtable slots, `dyn
ByteSource`/`SeekSource`/`RowSink`/`ListSource`/`embassy_usb::Handler`/
`RequestHandler`, the index sort comparators, the pictures `cancelled`
fn; `dyn GlyphSource` bound 0 B: no implementation is linked, asserted).
Recursion: core's `str::slice_error_fail_rt` (bounded, listed); the panic
handler's own cycle is gone (below).

**Fixes** (no behaviour change; bytes off the chains they were on):

* `Shell::with_root` = `unentered` + `enter_root` (orpheus-ui); `new_ui`
  builds the `Ui` without entering, `ui::enter_root` enters in place.
  With the HMAC split below, the thread context went 10,736 → 7,480 B (the
  4.8 KB temporary was under the root `enter`, which the graph cannot tell
  from any screen's; `new_ui`'s frame is 2,904 B now and calls nothing deep).
* `Shell::replace_top` (not inlined): the new screen's temporary (928 B
  frame of `rebuild`) is no longer under its `enter`.
* `orpheus_index::card::spin_in` (future built in its own frame) for all
  file-system calls of `CardIo`: `Inner::slot` 2,560 → 344 B (its four
  futures had been in one frame); the open future itself is 1,496 B.
* `media::with_io` → `io_session` (not inlined): `Server::step` 1,280 →
  720 B; `firmware::frame::Own` (a future polled in its own frame) on
  every await of `Server::serve`: its frame 2,352 → 32 B (the index path
  under it fell out of the top).
* `orpheus_otp::otp::mac` not inlined: `hotp` 1,576 → 104 B (three HMAC
  states had been in one frame); `Fw::auth_entry` not inlined (Authenticator
  row loop 992 → 752 B).
* `Fw::derive` split into `derive_start` / `derive_finish`: 4,240 → 304 B
  frame (the finish future, 4,008 B, is now the UI's deepest frame).
* `VorbisDecoder::load_setup`, `audio_packet` not inlined: `step` 1,904 →
  808 B.
* The panic handler (`atj2157_hal::panic`, `log::LineBuf`) formats
  without indexing or string patterns that can panic: it was on a cycle
  with its own bounds checks (unbounded recursion until the stack guard),
  and its tail under every chain is ~300 B shorter. A bug fix with no host
  test (HAL): the regression guard is `stack --check`, which fails on an
  unbounded cycle.

**Not done / remaining depth**: the RT context is 14,856 B, of which
9,624 B are three frames in `audio.rs` that this package left alone for
AU-PERF: the audio task closure's poll frame (5,840 B: `sub sp, #0x1688`),
`play_file`'s (2,296 B) and `seek_to`'s (1,488 B). Below `play_file`, five
paths are within 800 B of each other (seek via the Vorbis set-up 6,656 B,
`Server::step` index rows 6,496, `orpheus_codecs::open` (Ogg page copies:
`header_packets` 1,592, `peek_packets` 1,240, `assemble` 1,256),
`Server::serve` 6,160, decode 5,872), so more cuts there give little each.
The thread side: `derive_finish` (4,008 B, the vault's `finish` future
holding the PBKDF2 job and the seal) and the HOTP path (`mac<Sha512>` 1,232
+ `compress512` 1,448; sha2's `compact` backend would also slow the
PBKDF2 unlock). Init: `init_chain`'s 11,192 B is safe today (RT and USB
do not exist yet) but is the largest frame in the image. **≥ 4 KiB
margin needs ~4.9 KB more**; halving the audio task closure's frame and
`play_file`'s would get most of it.

**Budget**: `stack_static_margin_bytes = -1536` — a ratchet (fails on a
regression of more than ~650 B, or 255 B of guard-alignment luck), not a
proof that the stack is enough. Raise it as the frames shrink.

Gate: host fmt, clippy `-D warnings`, `cargo test --workspace`; fw fmt,
clippy; `xtask build`, `check-resident` (57 sections from 19 roots, none
paged), `size --check` (resident 79,963 B), `stack --check` passed. Not
run: `dev …` (the lead owns the player).

**Lead checks** (player): `cargo xtask dev fuzz-ui --replay
tests/regress/ui-device/stack-seed8-340.keys` (expect more than 1,912 B
free: the UI path lost ~3.9 KB statically, the RT index path ~2.5 KB);
the suite's stack line (`dev hil` INFO stack free); the Authenticator
(codes, unlock, change PIN: `derive`/`hotp` were split), a Vorbis seek,
Files browsing during playback (`with_io`/`spin_in`), a panic message in
the fault record if one happens (the handler's formatting was rewritten).

### 2026-11-25 — on-device UI fuzzing (VER-8, lead)

* `fuzz-ui --seed 8 --minutes 30 --play --settings`: **stack finding** —
  stack free **1,912 B of 22,248 B** (budget 2,048 B) after a 340-step
  sequence with settings changes (`tests/regress/ui-device/stack-seed8-340.keys`;
  did not reproduce with the last 20–320 steps). The stack region itself
  shrank from 23,768 B (VER-7 suite) as the resident image grew
  (78,684 → 80,012 B; the T-8 profiler alone +656 B). Open: find the deep
  path (after AU-PERF, which changes the audio frames), reduce it, and
  consider building the profiler only into profiling builds.
* `cargo xtask dev fuzz-ui --seed 7 --minutes 30 --play`: **no finding**
  — 2,720 key steps (ui_keys grammar, no Power) over all screens with music
  playing, 136 oracle checks (fault record, resets, stack, underruns,
  card writes, UI state).

### 2026-11-25 — T-8 on the player: Pictures-while-playing profiled (lead)

* Merged T-8 (`dda322b6`); the profiler is behind the HAL feature `prof`
  (firmware only — its T2 vector overflowed the NOR payload's `.bss` by
  232 B and staging failed); `xtask build` now builds the payloads too.
  U-TMR-1 answered: T2 at 1 kHz counted 15,041 ticks in 15,041 ms.
* Fault profile of `browse pictures` with MP3 playing (140 s, 1 in 4096
  logged): 2,981 line loads/s; the faults are mostly the **playback path**
  (audio task closure, `play_file`, `AnyDecoder::decode`,
  `Chain::active`, `Mp3Decoder::find`), not Pictures. `play-block` packs
  to 77 lines (3× the 24-line budget): hot code is scattered inside huge
  async closures.
* A generated 24-line `playback-hot` cluster changed nothing (3,022/s,
  9 underruns from slow fills) and was reverted.
* PC profile (135 s): 0 % idle; handler 62 % (decode 12.8 %, audio task
  9.4 %, `play_file` 9.4 %, `Chain::active` 6.8 % — paging charged to a
  trivial function, MP3 synth 6.8 % resident); thread 38 % with
  **bit-banged I²C 7.9 %** (sensor polling).
* Next (worker AU-PERF): move the per-block path out of the async
  closures into small named functions packed into a cluster that fits;
  make per-block predicates cheap; stop the I²C busy-wait.
### 2026-11-25 — AU-PERF: compact playback block path, I2C bus timing (worker, no hardware)

Problem (lead, T-8 profiles on the player, MP3 48 kHz from a shuffled
queue while Pictures is browsed, soak `mixed-ring-seed1-7` "browse
pictures"): ~3,000 NOR line loads/s, 0 % idle, thread mode starved,
underruns from slow refills; faults dominated by the playback path; the
`play-block` cluster 77 lines; 7.9 % of the time in `SoftI2c`.

* **Why it thrashed** (from the capture's own ELF, build `d857bbc5`, fault
  addresses through `llvm-symbolizer --inlining`): the code cache is
  **direct-mapped** (a line's place is its VA modulo 32 KiB, 02 §3.1). The
  card task's 37 KB state machine resumed every DAC half through five of
  its lines, two of them 32 KiB apart (the same physical line);
  `play_file`'s per-frame push loop (inlined `PcmOut::push`, 12 KB
  function) shared physical lines with the MP3 frame code inlined in
  `AnyDecoder::decode`; `Chain::active` (656 B, paged, 2 lines) was
  reloaded on every 256-frame DSP piece (≈ 375 loads/s alone). Also
  found: the card task's poll function had a **5,840 B stack frame**,
  taken on every wake-up.
* **Block path** (`fw/firmware/src/audio.rs` "the block path"):
  `play_block` (one pass: request poll, decode, DSP + PCM ring, refill,
  chores, logs) and the rare paths `head`, `pause`, `chores` are futures
  polled through `Apart` (a wrapper whose `poll` is `#[inline(never)]`,
  so each state machine is a function of its own); per-block work is in
  non-async functions with stable names: `decode_timed`, `begin_block`,
  `after_block`, `chore_due`, `reader_check`, `PcmOut::{put, wrote_half,
  after_half}` and `Room` (the wait for a half). The per-frame push is no
  longer async: `orpheus_audio::pcm::Feeder` (host crate) writes a block
  piece by piece and stops after a frame that finished a half; the loop
  awaits only there. The task is `audio` → `Apart(start)`, then
  `Apart(idle)` / `Apart(player_request)` in turn (the idle loop moved
  out unchanged: player requests and card hand-overs return to the task);
  `player_request` → `Apart(request_flow)`, `Apart(run_player)`;
  `play_item`'s rare awaits apart. Order of every step kept (two
  equivalences checked by host tests below; the rest by reading).
* **Codecs**: every `Decoder::decode` (MP3, FLAC, WAV, Speex, Vorbis) is
  `#[inline(never)]`: `AnyDecoder::decode` is a dispatcher (inlined into
  `decode_timed`). The MP3 one (8,722 B) is excluded from residency
  (`resident.toml`: it was paged inside `AnyDecoder::decode` before).
* **DSP**: `Chain` keeps the stage mask while nothing can change it but a
  setter (no ramp moves, the limiter's bit not decided by its envelope);
  every setter clears it; `debug_assert` in `run` compares it with
  `active()` on every block of every host test. The firmware calls the
  resident `Chain::process` again (`process_i16` had been inlined into
  paged code).
* **Player**: `Player::position` (the `Position` arm of `handle`, now
  called by it) and `Server::position` (firmware): the per-block position
  without `feed`'s sleep-timer checks and `Flow::of`.
* **hot.toml**: `steps-sample` (4 lines), `mp3-frame` (11: MP3 decode,
  `find`, `MpegHeader::parse`/`frame_len`, the ring's `ByteSource`),
  `play-block` (15: the wake-up's poll functions and the block path),
  `play-fill` (6: the card read path), `flac-frame` (9). `mp3-frame` +
  `play-block` + `play-fill` = 32 lines, each physical line once;
  `flac-frame` lands on `mp3-frame`'s lines, `steps-sample` on the end of
  `play-fill` (the least frequent). New cluster key `fits_with`: pagelink
  warns when clusters that run together share a physical line (host test
  `regress_clusters_that_run_together_share_no_cache_line`). The probe
  patterns of `gapless-join` no longer take `MpegHeader::parse`.
* **Predicted cost per block** (MP3, from the layout): a DAC half (32 ms,
  1.33 MP3 frames) touches at most the 11 + 15 lines of `mp3-frame` and
  `play-block`, plus `play-fill`'s 6 on a refill (one half in six at
  320 kbit/s), all on distinct physical lines: no self-eviction. Worst
  case (the UI evicted everything between halves) ≤ 26 loads per half ≈
  810 loads/s + ≈ 30/s for refills; best case ~0. **Coverage of the
  measured fault set** (`20261125-092936`, 130 logged faults, 109 loads,
  mapped by function and inlined source line onto the new code): 11.9 %
  of the loads gone (`Chain::active`; `Flow::of` re-maps), 15.6 % now in
  `mp3-frame`, 23.9 % in `play-block`, 0.9 % `play-fill`, 0.9 %
  `steps-sample`: **53 % of the loads are now in the conflict-free set or
  eliminated**; 4.6 % `View` (still in `opus-frame`), 42 % other (UI
  tasks, the card task's pictures/library code for the Pictures app,
  settings lookups, USB, Bluetooth). Note: `cargo xtask prof DIR`
  symbolises a saved capture against the *current* `layout.json`; after a
  rebuild the old addresses land on unrelated code, so the coverage above
  was computed against the capture's own ELF (main checkout
  `target/orpheus/firmware.elf`, scratch script, not committed).
* **I2C** (`atj2157_hal::i2c_soft`, `delay`): who — the `sensor` task's
  25 Hz sample (Count in background "While on" is the default), one
  `write_read` of 1 + 6 bytes = 181 half periods (81 SCL clocks) =
  4,525 half periods/s. Why 8 %: each half was `delay::spin_us(5)`, and
  `spin_us` spins about **3× as long** as asked (`cortex_m::asm::delay(n)`
  0.7.10 runs `n` loops of 3–4 cycles; it is given `core_hz / 1 MHz × us`):
  ≈ 15 µs a half, ≈ 2.7 ms of bus per sample, ≈ 68 ms/s ≈ 7 % plus the
  GPIO overhead — the PC profile's 7.9 %. The "100 kHz" bus ran at ≈ 30 kHz
  (01 §9 corrected). Now each half is timed on T0 (`delay::wait_us`: at
  least `half_us`, bounded so a stopped counter cannot hang; interrupt
  time counts towards it) and the clock-stretch limit is 1 ms on T0:
  ≈ 0.9–1.0 ms a sample, ≈ 2.5 % (predicted). The accelerometer has no
  known FIFO (U-ACC-1), the step detector needs its 25 Hz, so neither the
  rate nor bursts change. `acc bus <µs>` sets the half period until reboot
  (`acc bus 15` ≈ the old timing, `acc bus 2` ≈ 1.1 %: both chips are
  400 kHz parts and the stock firmware drives the accelerometer faster,
  fm-i2c-imu.md §1.3); `steps status` reports `bus_us_avg`, `bus_us_max`,
  `bus_load_cpct` (counters in SRAM0: the SRAM3 blocks are full). The
  tuner's bus (same driver, `fm`) also runs at a true 100 kHz now.
  `spin_us` itself is unchanged (its callers were proven with the longer
  waits); its doc says what it does. New unknown U-I2C-1.
* **Host tests** (4 new): `orpheus_audio::pcm` feeder = the straight loop
  for any wait pattern, piece size, channels, decimation, odd lengths;
  `orpheus_dsp` `settled_stage_mask_follows_every_change` (1,500 random
  setter/process steps; each setter's invalidation removed in turn was
  caught, except `set_format`/`reset` whose clearing is implied); `Player`
  `position_is_what_handle_does_with_a_position` (A–B included); xtask
  `regress_clusters_that_run_together_share_no_cache_line`.
* **Numbers**: resident 80,012 → 80,068 B (+56: `Chain::process` +40 for
  the kept mask, `.rodata.cst16` +16); `.bss` 124,040 → 123,808 B (card
  task future 7,592 → 7,312 B; +12 B bus counters); paged code 857,962 →
  857,864 B, 106 code + 18 rodata pages, slot 1,101,824 B (unchanged).
  **Stack on the playback wake-up** (prologue frames, RT on top of thread
  mode): 6,620 B (task 40 + card task 5,840 + `play_file` 36 +
  `AnyDecoder::decode` 704) → about 1,500 B (36 + 128 + 376 + 8 + 36 + 200
  + 56 + MP3 decode 664); the idle loop's frame is 2,032 B.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace`; fw `fmt --check`, `clippy
  --workspace -D warnings`; `xtask build` (payloads too), `check-resident`,
  `size --check`: pass (`cargo test --workspace`: 1,097 passed). Not run:
  any `cargo xtask dev …` (the lead owns the player), HIL.
* **For the lead (player)**, after staging this build:
  1. `cargo xtask dev prof --faults 120 --during "soak --replay tests/regress/soak/mixed-ring-seed1-7.soak" --only "browse pictures"`:
     line loads/s well below 2,981 (expected ≈ 1,300–2,000: the playback
     part ≤ ~800, the UI's unchanged), no `Chain::active`, `mp3-frame`/
     `play-block`/`play-fill` within budget in the cluster report.
  2. The same with `--pc 120`: idle above 0 %, handler-mode share below
     the 62 % (expected ≈ 35–45 %), `SoftI2c` from 7.9 % to ≈ 2–3 %,
     no `pcm: head … ub=fill` lines in `log.txt`.
  3. With nothing playing: `dev cmd 'steps status'` → `bus_us_avg` ≈
     1,000–1,100, `i2c_errors` unchanged; `dev cmd 'acc read'` ≈ 1,000 mg
     at rest. Optional (U-I2C-1): `acc bus 15` (≈ 3,000 µs, the old cost),
     `acc bus 2` (≈ 500 µs; then watch `i2c_errors` for a minute).
  4. `playdir` of an MP3 and a FLAC folder: `status` underruns 0, `dec:`
     line loads per track lower; the radio once (`fm`), its bus is faster.

### 2026-11-25 — plan: M18 jack tools (owner's request)

* New milestone M18 (08; spec 05 §7.17–7.19; PLAN item 17): signal
  generator, IR remote through the owner's jack IR adapter (anti-phase
  stereo carrier at half frequency, or 96 kHz; NEC/Samsung/SIRC/RC5/RC6/
  Kaseikyo/JVC/Sharp/raw/Pronto; remotes from `/IR/`), and peripheral
  control (gate, pulses, servo PWM, tone keying, serial over audio, CV)
  with device profiles from `/Peripherals/`. WPs JACK-0…3; new unknowns
  U-AUD-15 (output swing/DC/impedance) and U-AUD-16 (96 kHz for carriers).

### 2026-11-25 — T-8: profilers and the `hot.toml` loop (worker, no hardware)

Why now: browsing Pictures while music plays runs at ~0 % idle and
~3,000 line loads/s, the cover search reads ~30 entries per 250 ms with
~900 loads; Hybrid Opus and MP3 page more than they should. As built: 07
§5.6 "As built (T-8)".

* **RAM, honestly**: the 2 KiB at `0x307000` (SRAM3, after the PCM ring)
  were reserved for the persistent log ring, which has always run in SRAM0
  (`logring::ALT_BASE`, `0x137400`); nothing used them. They are now the
  **profiler block** (`orpheus_proto::mem::SRAM3_PROF`, layout in
  `orpheus_proto::prof`). No mode-pool partition is borrowed (Pictures
  while playing uses the whole decoder arena for the art, so borrowing
  would change what is measured) and nothing comes off the stack (≈ 5.8 KiB
  free at the soak's peak). SRAM0: +8 B of flags.
* **Thinning instead of draining**: 2 KiB hold 500 PC samples or 250
  faults. A capture started with `on` keeps a uniform sample of the whole
  run: when full it keeps every other entry and doubles its stride (host
  test: entry *i* is event *i* · 2^shift for any length); `burst` keeps the
  first 500/250 complete. No draining during the run: the scenario starves
  the USB task, and a reader would perturb what it measures.
* **PC sampler**: timer T2 (first use; [DS] only: U-TMR-1, new in 01 §9),
  own vector entry at **level 0** so critical sections and the level-1
  handlers are sampled too; a tick during a line load lands on the faulting
  instruction (paging time charged to the code that faulted). Resident:
  entry 18 B, handler 74 B.
* **Fault log**: `pager::orpheus_pager_fault` → `prof::log_fault` (84 B)
  per served fault while on; when off, one load and one branch (checked in
  the disassembly: `ldrb` of the flag, `cbz`). Ring push shared by both:
  310 B. Two words per fault: address, faulting PC, load/re-map,
  data/fetch, time (1,024 µs units, 18 bits, unwrapped by the host).
* **Commands**: `prof pc on|burst [µs]`, `prof faults on|burst`, `prof
  off`, `prof status` (answered by the USB task at once).
* **xtask**: `dev prof --pc S | --faults S`, `--during "<dev subcommand>"`
  (same process, sequentially; whitelist of subcommands that leave the
  firmware running and write no NOR); the soak calls `dev::prof::hook`
  after its prologue, around every action and before its final reboot,
  `--only TEXT` limits the capture to the first action containing TEXT. A
  build-id mismatch between the player and `layout.json` is refused
  (`--any-build`). Reports (text + CSV) and the raw capture in
  `target/orpheus/prof/<stamp>/`, plus `log.txt` (the log ring at the stop;
  `art: cover search` lines are printed). `--update-hot NAME` writes a
  `[[cluster]]` (≤ 24 code lines, ≤ 8 rodata lines by default, skipping
  sections owned by other clusters, hottest first). `cargo xtask prof DIR`
  reports a saved capture again (no player).
* **Host tests** (19 new): ring thinning, burst, entry round trips, parse
  errors (`orpheus_proto::prof`); symbolisation against a hand-built ELF32
  and layout sections, PC and fault aggregation, windows, time unwrap,
  cluster coverage, the generator's budget, ownership and order, the
  `hot.toml` rewrite (replace, append, idempotent, comments kept)
  (`xtask::profile`); the hook's start/stop decision, options, `meta.json`
  (`xtask::dev::prof`). A synthetic capture over the real `layout.json`
  went through `cargo xtask prof DIR --update-hot`, and pagelink packed the
  generated cluster (25 sections, 13,396 B = 14 lines; the generator's
  conservative estimate 13,451 B); `hot.toml` restored afterwards.
* **Seen on the way (not changed)**: cluster `play-block` packs 25
  sections into **77 lines** (78,036 B), three times the 24-line budget:
  `AnyDecoder::decode` with the MP3 frame code inlined and the card read
  path. The fault report flags clusters over budget; a measured profile
  should decide what of it stays.
* **Numbers**: resident 79,356 → 80,012 B (+656 B; budget 81,920 B),
  `.bss` 124,032 → 124,040 B, 106 code + 18 rodata pages (105 before: the
  `prof` command and its status text are paged),
  slot 1,101,824 B.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace` (debuginfo off); fw `fmt --check`,
  `clippy --workspace -D warnings`; `xtask build`, `check-resident`,
  `size --check`: pass (`cargo test --workspace`: 1,092 passed). Not run: any `cargo xtask dev …` (the
  lead owns the player), HIL, brand/site.
* **For the lead (player)**: stage this build, then
  `cargo xtask dev prof --faults --during "soak --replay tests/regress/soak/mixed-ring-seed1-7.soak" --only "browse pictures"`
  and the same with `--pc` instead of `--faults`; check U-TMR-1 once with
  `dev cmd 'prof pc on 1000'` … `prof status` … `prof off`. Then
  `cargo xtask prof target/orpheus/prof/<stamp> --update-hot pictures-browse`,
  review the `hot.toml` diff, `cargo xtask build`, stage, and profile again:
  the fault report's "pager counters" line (line loads/s over the Pictures
  step) and the `art: cover search … ms, N entries, … loads` lines should
  improve. Open: whether 250 thinned faults per capture are enough (take
  two or three captures, or a `--burst` one for the working set).

### 2026-11-25 — VER-1: regression tests for every fixed bug (backfill) (worker, no hardware)

Owner's policy of 2026-11-25 (07 §3.1) applied to the bugs fixed so far.
The inventory is `docs/regressions.md` (new; one row per bug: date, fix
commit, test or on-device check, status).

* **Inventory**: 56 rows (59 product bugs; three rows hold two each) from
  this log, the CHANGELOG and the commit log, plus 10 test-tool fixes. 27
  rows were covered already: 19 by a host test (renamed `regress_…` where
  dedicated: USB-W1 ×4 in `ep0_model` and `liveness`, governor, FM id
  fallback, Ogg large pages, F-SET-1; else a `Regression:` doc line), 8 by
  fuzz inputs in `tests/regress/` (the UI-8 marquee-under-menu input
  `58133401…` copied there from the corpus); 9 got a new host test; 20 are
  on-device only (each row names the check).
* **New tests** (each failed with the fix reverted locally, then passed;
  the SRAM3 one excepted, see its row): `mailbox::regress_fault_reboot_keeps_the_boot_counter`,
  `mem::regress_sram3_blocks_chained_not_overlapping`,
  `devchan::regress_playdir_during_playback_stops_the_track` (orpheus-proto);
  `adc::regress_mic_front_end_is_the_left_side` (atj2157-pac);
  `proto::regress_bt_volume_keys_act_at_the_dac` (orpheus-bt);
  `regress_every_app_is_on_the_players_home`, `regress_about_hardware_row_fits`,
  `id_tests::regress_service_kinds_screen_ids_setting_ids_unique` (orpheus-apps;
  the last reads the sources, so a new `other::` kind, `data::` tag, screen
  id or `settings::id` is checked without being listed);
  `dev::soak::tests::regress_soak_now_playing_does_not_open_shuffle_all`
  (xtask, simulator rig).
* **Decisions moved into host crates** (pure refactors): `mailbox::Reboot`,
  `fault_reboot`, `deliberate_reboot` (HAL `wdt`); `mem::SRAM3_*`,
  `sram3_next` (drain, sensor, fm, rec, bsp card counter, PCM ring);
  `devchan::stops_track_first` (`usbdev`); `adc::mic_mix_for_mic`,
  `ctl_for_mic` (HAL `adc`); `proto::dac_level`, `SINK_VOLUME_SENT`
  (firmware `apply_dac`, `bt_boot`); `App::on_player` (firmware
  `app_available`); `services::PLAYER_HARDWARE` (firmware and simulator).
* **Bugs found while writing the soak test (fixed, xtask only)**: (1) the
  soak's `clamp_lists` (b7949dc9) could not work on the player either: the
  test look reads every look setting, list roll-around included, as its
  default, and `goto` turns the look on before the held Up, so the wrap to
  Shuffle all stayed; the Now Playing step now turns the look off first.
  (2) The simulator rig parsed `settings test-set` ids as hexadecimal while
  the session sends decimal (the firmware's `parse_num`): every set was
  refused in the simulator, which is why the dry run passed. The new test
  fails with either part reverted.
* **Numbers**: resident 79,148 B, `.bss` 123,988 B, paged code 852,188 B,
  105 code + 18 rodata pages, slot 1,093,632 B — identical to `acc73592`
  built in the same worktree (a first cut of the `wdt` refactor cost 8 B of
  resident text; `reboot_as` inlined brings it back). `size --check`
  79,139 B.
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace` (1,054 passed); `cargo test
  --workspace regress_`: 19 tests; fw `fmt --check`, `clippy --workspace
  -D warnings`; `xtask build`, `check-resident`, `size --check`: pass. Not
  run: `xtask test golden` separately (the goldens ran in `cargo test`),
  any `xtask dev` command (the player was in a soak), HIL.
* **Open, not fixed** (listed at the end of `docs/regressions.md`): F-SET-2,
  F-UI-1, F-UI-4, the RTC stop, AU-0b, the first FLAC join, art dropouts,
  "Start original firmware" in the full layout, language packs on the
  player.

### 2026-11-25 — USB-W1: the USB wedge — root cause, self-recovery, liveness in the watchdog (worker, no hardware)

Analysis, design and decisions: 06-connectivity §2.6 (new); open hardware
questions U-USB-14 (01 §9).

* **Root cause (mechanism certain, trigger not determined).** (1)
  embassy-usb drops `ControlPipe::setup()` whenever a bus event wins its
  `select`; our driver kept the SETUP interrupt masked until its DMA read
  finished, so a drop mid-read left it **masked for good**: SETUPs latched,
  never read, a bus reset did not unmask it and VBUS loss did not
  re-initialise the controller — exactly "-71 on setup address" after
  re-plugging, cured only by a reboot. (2) The `Cancelled` → `Stall` step:
  when the host abandons an OUT whose data the device has not read and
  sends the next SETUP, the waiting EP0 OUT DMA drains the new SETUP as
  data and the old request's `CX_DONE` finishes the new transfer (the PEEK
  sees a status stage instead of data). Both reproduced with embassy-usb
  itself over a model of the EP0 hardware state
  (`crates/orpheus-usb/tests/ep0_model.rs`, "before" and "after"
  flavours). (3) The trigger, a device more than 2 s late on one request,
  is not identified (candidates in 06 §2.6); `usb delay` now produces it on
  demand. Ruled out: a panic in the USB task (would reboot), PEEK of a bad
  address (would fault or starve the watchdog), PEEK length handling
  (host-tested), LPM (not advertised).
* **Driver** (`atj2157-hal::usb`): drop-safe SETUP read (guard unmasks and
  aborts the EP0 DMA however the read ends); a bus reset re-arms EP0 and
  wakes every endpoint future (a bulk OUT read pending at a reset waited for
  ever: card reader dead until a personality switch); data/status stages
  overtaken by a newer SETUP are given up; `CX_COMABT` cleared (W1C) and
  counted at each SETUP read [FOTG210]; vendor DMA aborts (`DMA_EN = 0`);
  DMA waits end on a reset epoch, not the sticky `BUS_EVENTS` bit (which
  made a failing SETUP read spin without yielding); `Bus::poll` reports
  `PowerRemoved` after 0.75 s without VBUS, and the next `enable`
  re-initialises the controller fully; a SETUP while suspended counts as a
  resume. Test hooks `usb delay setup|data|status <ms>`, `usb recover`.
* **Self-recovery** (`orpheus_usb::liveness`, pure, 10 host tests): once a
  second from the `watchdog` task. VBUS present and a bus reset not answered
  within 2 s, or a SETUP unread for 2.5 s → recovery: the personality's
  `select` ends (`REINIT`), controller shut down, same personality on a
  fresh controller (USB Drive ends instead); backoff 0/5/15/30/60 s.
  Unplugged, charger, idle host: nothing ever happens.
* **Watchdog** (decided for, limited): `wdt::USB_WEDGED` stops
  `Health::check` feeding only if a requested recovery is not carried out
  within 10 s, or SETUPs stay unread after two recoveries — and only with
  VBUS, after this boot answered a SETUP, after 60 s uptime (never a failed
  start for stage 2). Unanswered resets alone never reboot.
* **Observability**: `STATS` group 0 words 24–30 (stalls, given-up
  transfers, recoveries, last error code + time, initialisations,
  `CX_COMABT`, liveness state; layout in `orpheus_proto::devchan::usb`);
  log lines `usb: recovery (…)`, `usb: wedged (…)`; `xtask dev stats` prints
  a `usb:` line. **Soak**: prints the counters after a channel loss,
  reconnects up to 4 times 3 s apart (a recovering player re-enumerates),
  and a loss with more re-inits and the same boot is a `usb-recovery`
  finding that is logged and reported but does not stop the run.
* **New tool**: `xtask dev usb-cancel [--rounds N]` (not run: no hardware)
  — per round, the next request held 2.5 s at each stage then the soak's
  PEEK at once, and 40 transfers cut by the host after 1 ms; INFO must
  answer again within 20 s each time; counters before/after.
* **Tests**: `orpheus_usb::liveness` 10, `ep0_model` 7 (embassy-usb 0.6
  over the model: both flavours answer; before: a bus event during the
  SETUP read deafens EP0 for good, a late status lands on the next
  transfer; after: drop-safe read, overtaken transfer given up, a reset
  re-arms EP0), `devchan` PEEK lengths/address and refusal, proto USB
  words, soak counters and self-recovery classification.
* **Numbers**: firmware resident 79,052 B (`xtask build`; `size --check`
  79,043 B, within the 81,920 B budget), bss 123,988 B: **RAM +128 B**
  (`.bss` 123,348 → 123,476 B, `.data` 16,340 B unchanged; `cargo build
  --release -p firmware` of `e91558eb` and of this commit, `llvm-size`;
  the watchdog task with the detector and 52 B of counters and flags; the
  main stack is 128 B smaller), 105
  code + 18 rodata pages, slot 1,093,632 B; `check-resident` ok, 48
  sections from 16 roots, the same 3 indirect branches (`on_irq` and the
  bus-reset path stay resident).
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace`; fw `fmt --check`, `clippy
  --workspace -D warnings`; `cargo xtask build`, `check-resident`, `size
  --check`: pass. Not run: HIL, `dev soak`, `dev usb-cancel` (no
  hardware).
* **Lead's device test plan** (one process at a time; nothing needs the
  case opened; ends with the player running normally):
  1. `cargo xtask build`, `xtask dev stage` (or `dev update`), reboot;
     `xtask dev stats`: the `usb:` line shows `re-inits 0`.
  2. `xtask dev usb-cancel --rounds 3`: expect PASS; note `aborts`,
     `comabt` and `re-inits` (0 re-inits: the driver fixes alone handled
     every abandoned transfer; > 0: the liveness recovery did). A FAIL
     saying the player stopped answering: wait two minutes (watchdog),
     then stop and ask (rule 7).
  3. Re-plug the cable five times; `dev stats` after each: `re-inits`
     grows by one per plug (`vbus-reinit`), INFO always answers.
  4. `xtask dev cmd "usb recover"`: the player re-enumerates within ~1 s
     (`recover-setup`, `re-inits` + 1).
  5. `dev soak mixed --minutes 30 --setup --seed 1` again: a channel loss
     must now end in `usb-recovery` (the run goes on) or not happen.
  6. User check 28 (re-plug, host sleep, charger, idle hour).
* **Open**: what made the device 2 s late in the first place (watch
  `data-timeout`/`overtaken` in the counters during the soak, and the
  settings/clock words); U-USB-14 (is `CX_COMABT` set, is the W1C needed;
  does `DMA_EN = 0` matter); the ZLP after an exact multiple of 64 bytes
  (U-USB-2, unchanged).

### 2026-11-24 — T-6 (stage2): automatic fallback to the next slot (worker, no hardware)

Owner's decision 2026-11-24: stage 2 falls back to the other slot by
itself when a valid image keeps crashing; installed with `install-stage2`.
Design: 02 §2.5 "Automatic fallback (as built, T-6)".

* **Decision** (`crates/orpheus-proto/src/boot.rs`, `decide`): pure,
  `#[inline(always)]`, used by stage 2 and the tests. Counter word
  `"BMY" << 8 | tripped << 4 | tries`; candidates = valid slots A, B,
  dual, then — only once something tripped — superseded slots in the same
  order; a slot that failed three starts in a row is skipped; nothing left
  → USB recovery with the counter cleared. Boot-info word path 1 carries
  `tripped << 4 | tries` and the slot index (low byte unchanged, so the
  installed firmware still reads its slot); path 4 (mailbox ADFU) keeps
  the last start's low 16 bits for `xtask dev slots`; path 5 = every
  candidate tripped, 6 = no image, 7 = stage-2 panic.
* **Superseded slots as the last resort** — beyond "next valid slot" in
  the brief, because after an update A → B the old A is only superseded
  (UPD-1), so without this the fallback could never undo a bad update; a
  B → A update leaves B valid and is covered either way. They are
  candidates only after a trip, so a normal start is exactly as before.
  The firmware reads a recorded superseded slot's header with the magic
  put back (`SlotHeader::from_bytes_or_superseded`; `find_slot`,
  `running_build`). **For the lead**: firmware older than this commit
  started from a superseded slot finds no header (no paging) and would
  itself crash → USB recovery, as today; drop the second pass if the owner
  prefers (3 lines in `decide`).
* **Stage 2** (`fw/boot/stage2`): calls `decide` with its header/sum check
  as the closure; depends on `orpheus-proto` now (addresses, `SLOTS`).
  Never writes NOR; key and mailbox ADFU unchanged and first. Nested
  loops in `decide` let the optimiser drop the bounds-check panic path.
  **Size: 876 B → 964 B of 1,024** (`__stage2_end − 0x102400`, release,
  `opt-level = "z"`; first cut 1,004 B).
* **Bug fixed** (`atj2157_hal::wdt::reboot_after_fault`): it called
  `reboot`, which clears the counter, though its comment said fault reboots
  keep it — so a firmware that faulted at start-up never tripped the
  counter, only hangs did. Now `reboot_keep_count`.
* **Firmware**: `persist::boot_info`, `arm_crash_test`,
  `take_crash_test` (HAL, resident); `main` runs the crash test right
  after the pager starts, before any task (no NOR write can be cut short);
  test commands `boot status` (`slot B, start 1, after A failed`, plus an
  armed crash test) and `boot crash-test` (the running slot fails its next
  three starts; at most three per arming; another slot's start disarms;
  `mailbox::crash_step`, host-tested). About shows *Firmware slot* and,
  after a fallback, *Failed to start* (`SystemService::boot_slot`, default
  `None`; strings `FwSlot`, `SlotFailed` appended; ru/ja in the packs).
* **Host tools**: `xtask dev slots` prints stage 2's record (`started
  firmware from slot B, start 1 of 3, fallback: A failed 3 starts`, or
  for path 4 "the start before: …"); `xtask dev results` prints it too
  (`describe_boot`, tested). `orpheus_update::slots::stage2_decide` models
  the whole decision over a NOR read function.
* **Tests**: `orpheus_proto::boot` 10 (A valid / B empty, A tripped → B,
  B tripped → superseded A, both tripped → ADFU and A again after it,
  invalid and superseded slots skipped, dual slot, counter after a good
  boot, garbage words, call order = header contract); `mailbox` crash-test
  steps and boot-info fields; `slot` superseded header; `orpheus-update`
  `fallback_after_an_update_that_keeps_crashing` (real images, a real
  A → B update, crash loop: B ×3 → old A ×3 → ADFU; a damaged superseded
  image never starts; then an update from the fallback writes B);
  `xtask` `boot_descriptions`. Goldens: 6 new `app_about_fallback_*`
  (looked at; ja strings changed to glyphs the CJK font has).
* **Numbers**: stage 2 964 B of 1,024 (was 876 B); firmware resident
  78,868 B of the 81,920 B budget, 104 code + 18 rodata pages, slot
  1,085,440 B (`xtask build`); `check-resident` ok, 3 indirect branches
  as before (a first cut with a function pointer in `find_slot` added a
  fourth; replaced by direct calls).
* **Gate**: host `fmt --check`, `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace` (all pass; golden images included);
  fw `fmt --check`, `clippy --workspace -D warnings`; `cargo xtask build`,
  `cargo build --release -p stage2` (the linker's 1 KiB assert holds),
  `cargo xtask check-resident`, `cargo xtask site --check` (manual,
  dev guide and Install page say the player falls back first): pass. Not
  run: `test all` as a whole (fuzz smoke), `brand --check`, `size
  --check`, HIL (no hardware).
* **Lead's device steps** (in this order; one process at a time; nothing
  needs the case opened; it ends with the player running slot A
  normally):
  1. `cargo xtask build` (builds the firmware, the `.ofw` and stage 2).
  2. Put this firmware into **both** slots first, so the fallback target
     knows the new boot-info encoding: `cargo xtask dev backup-verify`;
     `cargo xtask dev stage target/orpheus/firmware.yfw` (slot A);
     `cargo xtask dev reset`; `cargo xtask dev update
     target/orpheus/orpheus-*.ofw` (same build into B, A superseded);
     `cargo xtask dev switch-slot`; `cargo xtask dev reset` (A again, both
     valid); `cargo xtask dev slots` → A valid, B valid.
  3. `cargo xtask dev install-stage2` (backup-verify inside; keeps the
     vendor record copy; the old stage 2 goes to
     `local/stage2-previous.bin`). `cargo xtask dev reset`.
  4. `cargo xtask dev cmd 'boot status'` → `slot A, start 1`. (Stage 2
     runs on every boot, so this also proves the new stage 2 starts A.)
  5. `cargo xtask dev cmd 'boot crash-test'` → `crash test armed: slot A
     fails its next 3 starts; reboot to begin`. `cargo xtask dev reset`
     (via USB recovery: the deliberate reboot clears the counter, ymon's
     reset starts stage 2). Expect three quick restarts of A (screen
     flashes, no Home), then B starts. Wait ≥ 30 s.
  6. `cargo xtask dev cmd 'boot status'` → `slot B, start 1, after A
     failed`; Settings › System › About shows *Firmware slot B* and
     *Failed to start A*; `cargo xtask dev slots` → "the start before:
     slot B, start 1 of 3, fallback: A failed 3 starts" (this one goes
     through USB recovery).
  7. Restore: `cargo xtask dev reset` → A starts (crash test spent and
     cleared, counter 0). `cargo xtask dev cmd 'boot status'` → `slot A,
     start 1`, no "after". Done; the player runs A normally.
  8. Optional, the real update case (superseded fallback): `cargo xtask
     dev update target/orpheus/orpheus-*.ofw` (from A: writes B,
     supersedes A, restarts in B); `cargo xtask dev cmd 'boot crash-test'`
     (arms B); `cargo xtask dev reset` → B ×3 → the superseded A starts;
     after 30 s `cargo xtask dev cmd 'boot status'` → `slot A, start 1,
     after B failed`, `cargo xtask dev info` answers (paging on from the
     superseded header). Restore: `cargo xtask dev reset` → B (healthy);
     `cargo xtask dev switch-slot`; `cargo xtask dev reset` → A, both
     valid, as after step 7.
  If after step 5 nothing answers for two minutes (USB recovery is the
  worst case: every candidate tripped), `cargo xtask dev reset` starts A
  normally (request spent); otherwise stop and ask (rule 7). Undo of the
  stage-2 install: write `local/stage2-previous.bin` back the same way
  (`install-stage2` of the previous commit).
* **Open**: firmware older than T-6 in a slot stage 2 falls back to as a
  superseded slot (no header → no paging); the counter forgets tripped
  slots on a healthy start, so a slot that crashes every start costs three
  failed starts per reboot (RAM only, no NOR writes — by design).
### 2026-11-24 — VER-12: long-soak tooling (worker, no hardware)

* **`cargo xtask dev soak <scenario> [--hours H | --minutes M] [--seed N]
  [--sim] [--report DIR] [--setup] [--battery] [--music DIR]…
  [--no-minimise] [--max-replays R]`**, `--replay FILE`, `--list`
  (`xtask/src/dev/soak.rs`). Six scenarios, each a seeded loop of weighted
  actions (weights, durations, oracle limits: 07 §5.10 "As built"). Every
  run starts with a reboot, `ui test-look on` and a settings test session,
  and ends with clean-up, the card root comparison and a reboot; every
  error after the start reboots too (unless the player stopped answering:
  rule 7).
* **Firmware test hooks** (the soak changes settings without touching the
  owner's): `settings test-session on` keeps every setting change (keys,
  `test-set`, radio presets and start station) in RAM — the end-of-run
  reboot restores the stored values; `settings test-session` reports the
  store as the boot read it (keys, CRC), compared before and after a run;
  `settings test-set ID VALUE` (Sound, Playback, Display without Home
  apps, Language, Performance, Files sort/hidden, Now Playing view; never
  Bluetooth, power, clock, USB, storage, alarms); `ui screen-off` (screen
  off without the key lock that Power would set); `status pos=`; STATS
  words 21 (DAC headroom low-water since the last read), 22 (failed card
  operations), 23 (failed pager line loads). +22 B of statics, stack −48 B
  (`__stack_bottom` 0x131a40 → 0x131a70), resident +32 B.
* **`[soak]` in `budgets.toml`**: sampling 10 s; line loads/s thrash
  > 7,000 for 3 samples, run mean ≤ 4,500 (measured ≈ 2,670–3,220 during
  48 kHz MP3); evictions/s 7,000 (not measured yet); underruns 0 outside
  10-s disruption windows, ≤ 6 inside; headroom ≥ 1 frame; card and pager
  errors 0; DAC drift ±2,500 ppm over ≥ 300 s (U-AUD-10's +1,500 ppm is
  inside it); uptime ±300 ppm; `--battery` stops at 3,550 mV; ≤ 24
  recordings per run.
* **Simulator mode and host tests** (14 in `soak.rs`): deterministic
  generation, actions round-trip through `.soak` text, generated settings
  inside the firmware whitelist (the host checks the firmware source),
  oracles pass a healthy stream and trip on each kind of bad data,
  underrun windows, `ddmin` on actions, all six scenarios 40 actions in
  the simulator clean (Home found every time, no power/storage/factory
  reset request), a planted bug found, replayed, minimised and saved, a
  planted two-action bug minimised to exactly those two. `xtask dev soak
  mixed --sim --minutes 30`: 24 actions, 197 samples, pass, ≈ 16 s.
* Gate: host `fmt`, `clippy --workspace --all-targets -D warnings`, `cargo
  test --workspace` (1,006 tests) pass; fw `fmt`, `clippy --workspace`,
  `xtask build`, `size --check`, `check-resident` pass. Not run: the
  player (host-only package; the lead runs the soaks), `brand --check`,
  `site --check`.
* **Open**: the queue check only sees `queue.bin`/`player.bin` listed (the
  restored queue is published only when something plays); an unplugged
  run cannot be host-driven (`--battery` = cable in, charging paused —
  whether that drains the cell is open); Bluetooth on/off is not churned
  (rule 6); the screenshot-per-checkpoint oracle and stream-ring high-water
  are not built; thrash and eviction limits are first guesses until the
  first runs.
* **Lead, first run**: stage a build with the hooks, then `cargo xtask dev
  soak mixed --minutes 30 --setup --seed 1`; read
  `target/orpheus/soak/…/summary.txt` and `index.html`; then `music`,
  `radio`, `browse-while-playing`, `settings-churn` (2 h each), `stress`
  (1 h), `mixed` (8 h, then 24 h).

### 2026-11-24 — UPD-1 (firmware update) and WEB-3 (browser installer) (worker, no hardware)

* **`orpheus-update`** (new, `no_std`, host-tested, 31 tests): the update
  file `orpheus-<version>.ofw` (384-byte header: `"OFW1"`, version text,
  build id, SHA-256, layout, ≤ 256-byte change list, CRC; then the YFW1
  slot image; `ofw::check` validates header + slot header against stage 2's
  and the pager's limits before anything is written), slot choice (running
  A → B and supersede A; running B or dual → A), the installer (erase target
  header sector → body with per-page read-back → SHA-256 + resident sum →
  whole read-back hashed → header without magic → magic → supersede old
  magic, bits only), stage 2's slot check as a model, the recovery switch
  (`slots::switch`), a simulated NOR with torn erase/program. Power cut
  before **every** operation × 3 torn patterns × (A→B, B→A, dual→A): stage 2
  always finds the old or the new image byte for byte, and a retry
  completes. Guard tests: forged targets at 0, `0x7E0000`, settings; the
  supersede word at sector 0 — all refused, nothing written. Design and
  details: 02 §2.5 "As built".
* **No RAM installer** (02 §2.5 planned one): in the full layout the update
  never writes the slot the firmware pages from, so the running firmware
  does it; the dual layout refuses (`NotFull`).
* **Stage 2 not changed.** The slot order A, B, dual plus "supersede the
  running slot's magic" switches slots without touching stage 2. But stage
  2 as built goes to **USB recovery after three failed starts, it does not
  try the other slot** (CHANGELOG's "falls back to the next slot" is not
  what `fw/boot/stage2` does). A new image that is valid but crashes ends
  in ADFU; `xtask dev switch-slot` or the Install page's Recovery switches
  back. Automatic fallback = a stage-2 change (skip the last-started slot
  once the counter trips) = an `install-stage2` step for the lead; not
  done here.
* **Player** (`fw/firmware/src/update.rs`): Settings › System › Update
  firmware (and OK on an `.ofw` in Files) → `UpdateScreen`
  (`orpheus-apps`, 33 goldens `update_*`, looked at): searching → files
  (version, size, "installed" by build id) → change list → "Install this
  version?" (≥ 30 % battery or cable) → checking (whole-file SHA-256,
  nothing written) → writing → verifying (bar, "Do not switch off") →
  "Update installed" → restart after 2 s; failures say why and "The
  installed firmware is unchanged". The card task does the work
  (`media::Req::Update`; Install waits for playback to stop), 4 KiB steps
  with a 1 ms timer between them (it runs on the interrupt executor).
  Reads for the installer go through 256-byte critical sections; erase and
  program through the HAL's guarded driver. `INFO.build_id` now reports the
  running slot's build id (`xtask dev info` prints it in hex).
* **Developer channel `UPDATE` (0x0D)** (`orpheus_update::wire`):
  `BEGIN`/`DATA` n/`COMMIT`/`ABORT` + 16-byte status; DATA is written
  inside the request (≤ one erase + 4 programs), the USB supervisor reads
  back 16 KiB per 20 ms after COMMIT. `cargo xtask dev update FILE.ofw
  [--no-reboot]` (check file, send, wait, reboot, compare INFO build id),
  `xtask dev slots`, `xtask dev switch-slot` (ADFU, after backup-verify).
  `cargo xtask build` also writes `target/orpheus/orpheus-<version>.ofw`
  (change list from `target/orpheus/changes.txt` if present).
* **`orpheus-host` refactor (WEB-3)**: one `async` `Transport` trait; BOT,
  ADFU/ymon (`adfu::Link`), NOR payloads (`nor`, payload bytes passed in),
  developer channel + update (`devchan::Link`) written once over it.
  Desktop: `native` (`nusb`, blocking front ends `Adfu`, `DevChannel`,
  `native::nor` keep xtask's calls); browser: `orpheus-install-web`; tests:
  `mock::MockPlayer` (ROM ADFU, ymon, nordump/norprog/norprog0 stand-ins,
  the firmware's developer channel and update over a simulated NOR; counts
  forbidden writes). Write guards: `write_full`/`erase_full`/`program_bits`
  only `0x1000..0x7E0000`; sector 0 only via `write_sector0`, which needs a
  `Sector0Permit` (whole-chip backup + confirmation); also the payload
  refuses. `nor::write_*` now compare first and skip equal sectors
  (resumable). `xtask dev restore vendor` passes its backup for the permit.
* **Flows** (`orpheus_host::flows`, 8 host tests over the mock): backup
  (whole chip read twice, JEDEC checked, mismatch refused), first install
  (stage 2 sector → sector 0 = stage 1 + vendor bytes → slot A body then
  header → erase the rest up to `0x7DF000`; refuses a non-stock backup and
  bad release files; interrupted at 5 points and resumed = same chip),
  restore (all differing sectors below `0x7E0000`, then sector 0, then the
  stage-2 sector; verify; vendor data never written), recovery
  (slots, switch, reinstall). Order of first install: boot record before
  the stock firmware is overwritten, so the player always reaches stage 2
  → Orpheus or USB recovery.
* **WebUSB / MS OS 2.0** (`orpheus_usb::webusb`): the developer interface
  carries the WebUSB platform capability (vendor code 0x57, landing page
  `…/orpheus/install.html`) and an MS OS 2.0 set (WINUSB + interface GUID;
  device level when alone, function subset in the keyboard personality);
  `GET_URL` answered by `DevChannel`. Host test drives embassy-usb's real
  control handling through a fake controller (BOS, URL, descriptor set,
  bcdUSB 2.1). Not in the card-reader personality (no vendor interface
  there). ADFU unchanged (vendor class 0xFF: WebUSB can claim it; Windows
  needs WinUSB once, the page says so).
* **Install page** (`site/pages/install.md`, `site/install/`: `app.js`,
  `usb.js` = the WebUSB transport, `mockusb.js` = the simulated player in
  WebUSB's shape, `install.css`): Update (release `.ofw` or a chosen file),
  restart into recovery, backup (download + IndexedDB; a stock backup is
  never replaced by a later non-stock one), first install and restore
  (backup + "I saved it" + boot-record confirmation), recovery (slots,
  start the other firmware, reinstall, restart), resume banner for an
  interrupted job, release files checked against
  `install/release/manifest.json` (`{version, files: {name: {sha256,
  size}}}`; none published yet: the page says so). Chromium only;
  elsewhere the command-line path. `install.html?demo` runs it all
  against the simulated player. From the **stock** firmware the browser
  cannot reach ADFU (the stock firmware is mass storage, a protected
  class; ADFU entry is a SCSI command): the page says to run `cargo xtask
  dev adfu` once.
* **Tests**: `tests/web/install.mjs` (Node, 7 tests: update + recovery
  switch, damaged file refused, backup + bad read, first install refused
  unconfirmed / installed / refused on Orpheus, interrupted + resumed,
  restore, transport stalls) passes; `--page` (headless Chromium 155, demo
  update through the page's own buttons) passes; `--device` (HIL, visible
  Chromium with a persistent profile `target/orpheus/install-hil-profile`;
  the first run asks to pick the YM03 once) **not run** (no hardware).
  `cargo xtask wasm` builds the installer into `site/install/pkg`
  (`[web] install_gzip_bytes` 300 KB; module 205 KB) and `--test` runs the
  Node suite. `xtask site` keeps `install/pkg` and `install/release`; the
  link check ignores `?query`.
* **Docs**: manual `docs/user/install.md` (page first, then tools), 02
  §2.5 as built, user check 27 (a first-time user updates from the page;
  then the on-player update).
* **Numbers**: firmware resident 78,676 B, `.bss` 124,648 B (build on
  `f484eeaa` + this work, before the rebase on `43903ea8`; slot 1,085,440 B, 104 + 18 pages). UPD-1's own
  RAM, from the map: `INST` 360 B (`.data`: `Option<Installer>`), `FILES`
  216 B, `CHANGES` 258 B, atomics 17 B = **851 B** (≤ 1 KiB); no other
  static grew (the BOS/MS OS buffers existed). `__stack_bottom` =
  `0x131A40`. Installer module 205 KB raw, 99 KB with the page gzip.
* **Gate**: host fmt, clippy (`-D warnings`, all targets), `cargo test
  --workspace` (rebased on `43903ea8`), golden images (33 new, looked at;
  `files_notice` unchanged: `.yfw` still shows the notice, `.ofw` opens
  Update), fw fmt, clippy; `xtask build`, `check-resident`, `site
  --check`, `xtask wasm` pass; Node suite and `--page` pass. `size
  --check`, HIL: not run.
* **Lead's device steps** (in this order; one process at a time):
  1. `cargo xtask build`; `cargo xtask dev backup-verify`; `cargo xtask dev
     stage target/orpheus/firmware.yfw` (the new firmware into slot A the
     usual way); `cargo xtask dev reset`; `cargo xtask dev info` → build id
     in hex.
  2. `cargo xtask dev update target/orpheus/orpheus-*.ofw` (same build
     into slot B through the developer channel): expect "written and
     verified into slot B", reboot, "running … from slot B". `xtask dev
     slots` afterwards (switches to ADFU): A superseded, B valid.
  3. `cargo xtask dev switch-slot` then `cargo xtask dev reset` → boots A
     again; `switch-slot` + `reset` once more → B. Then an update from B
     writes A.
  4. On the player: copy the `.ofw` to the card's top folder, Settings ›
     System › Update firmware → Install; watch the bar; it restarts.
  5. WebUSB: `cargo xtask site && cargo xtask wasm`, then `node
     tests/web/install.mjs --device --ofw target/orpheus/orpheus-*.ofw`
     (Linux needs the udev rule for 1209:0001; first run: pick the YM03 in
     the chooser). Check `lsusb -v -d 1209:0001` shows the BOS with the
     WebUSB and MS OS 2.0 platform capabilities, and that Chrome shows the
     landing-page notification.
  6. Optional, read-only: in recovery mode `node tests/web/install.mjs
     --device --backup` (backup through the browser; compare its SHA-256
     with `xtask dev nordump 0 0x800000`).
  Never run the page's first install or restore on the reference player
  without the owner.
* **Open**: automatic slot fallback (stage 2 change, above); a published
  release (manifest + `ymon.bin`, `nordump.bin`, `norprog.bin`,
  `norprog0.bin`, `stage1.bin`, `stage2.bin`, `firmware.ofw`: licences of
  the companion repository's payloads, D-1/D-3); the Windows WinUSB step
  for ADFU and the WebUSB permission pre-grant format are untested.
### 2026-11-25 — VER-7: the first full suite run on the player triaged; Card info; UI stack (worker, no hardware)

The lead's run `target/orpheus/suite/20261125-031534/` (5 of 15 journeys
passed, `stack free 2352 of 24232 B`). Each failure, its cause and the fix
(re-judged offline against the player's PNGs from that run with the new
expected frames and masks: every suite-side failure now passes; the
firmware ones need the new build):

| Shot | Cause | Kind | Fix |
|---|---|---|---|
| settings/usb, usb/usb_group | the focused label's marquee: it starts 800 ms after the focus moves; the player took longer from the key to the shot (the frame was in the end pause, "…en connected") | suite (timing) | a shot that does not match is read again for up to 6 s (the marquee rests at 0 for 1 s every cycle, with the ellipsis the simulator draws) |
| settings/system | the player runs the full layout: no "Start original firmware" (UI-2, D-2); the simulator's default is the dual layout | suite (precondition) | `appearance = "set:dual=0"` |
| system/about | labels are shortened to fit their values: the firmware's hardware string "Yoton YM03 (ATJ2157)" shortened "Hardware" to "Har…" (the simulator says "YM03"); the serial is the player's own | both | firmware hardware string "YM03" (as the simulator); the Serial label masked for this shot |
| system/card | `Fw::card_info` returned `None`: Card info always said "No card" | **firmware bug** | Card info: size and file system published at every mount (`audio::publish_card`), free space counted once in the card task's idle loop (`count_free`: exFAT bitmap ≈ 240 KB on the owner's 238 GB card; FAT32 FSINFO, else one FAT scan) then kept by the volume; `state::CARD_*` atomics; 0 free shown for the moment it is being counted |
| files/files_menu, pictures/pictures_menu (4 px) | the listing's scroll thumb shows beside the sheet's rounded top corners (y 40–42) | suite (mask) | the "listing behind the sheet" mask covers y 14–42 |
| music | the Music root's rows depend on the player (library ready: no "Building library…" row; Now playing / Resume after any playback): `down*8` landed on Bookmarks | suite | Folders reached from the end (`up*2`, the list rolls round); Music's rows masked from y 33 |
| nowplaying/np, `status` | the fixtures (6 s + 3 s) had ended: the journey took 23 s; Now Playing said "Nothing playing" (the 40 px are the play icon missing) | suite (fixture) | fixtures 30 s each (44.1 kHz mono, deterministic, ≈ 100 s to upload); `cmd: status ~ ^playing` before OK on Music (else OK would start Shuffle all); a failed `cmd:` now stops the journey, and a journey stopped early sends `stop` |
| steps (468 px) | footer "No clock: no history": the player's clock is not set | suite (state) | variants `clock` / `no-clock` (`set:clockvalid=0`); the suite does not set the clock |
| authenticator (7425 px) | **"Vault damaged"**: the vault sector 0x7DE000 holds no vault but the bytes that were there before the full-layout switch (`local/nor-before-full.bin`: 3,320 non-0xFF bytes in 0x7DE000–0x7DF3FF; the stock dump has 0xFF there), so the header has no magic → `Status::Damaged` | state (not touched) | variants `locked` / `none` (set-up note) / `damaged`; the shot is `auth`; the vault is never written by a test. User decision below |
| usbdrive/home (354 px) | the owner's card has no `/Images` (dim row + "No Images folder…"); `/.orpheus/images` exists (USB-6 test) | suite (card) | variants `both` / `test-only` / `images-only` / `neither`; simulator `set:driveimages=0`, `set:drivetest=0` |

* **Suite** (`xtask/src/dev/suite.rs`): `variants = [{ name, sim }]` per
  journey (expected `<shot>.<variant>.png`, best match reported as
  `shot@variant`, `variant` in `summary.json`); shot re-reads
  (`SHOT_RETRY_MS` 6 s, wall clock on the player, simulated time in the
  dry run); the reference epilogue as the player's (long Back, a short one
  first if a dialog stays open: the set-up note); `FIXTURE_SECS` 30. Tests:
  `a_shot_matches_the_variant_the_player_is_in`, report shows the variant,
  fixture length and determinism.
* **Stack** (static, direct calls from `llvm-objdump`, one-time start-up
  and panic paths left out; the audio task runs on the RT executor at
  interrupt level 2, so its depth adds to whatever the UI task is doing):
  * UI task worst path **13,640 → 7,608 B**. (1) The settings-store mount
    moved into `ui::mount_store` (not inlined, `block_on`: the NOR driver
    blocks): its 1 KB `Option<Store>` passed through the UI task's poll
    frame three times; poll frame 4,776 + 40 → 904 B (`mount_store` 3,712 B
    once at boot). (2) `AppScreen::event` (3,448 B: all 26 screens' handlers
    inlined into one frame) and `enter` now call `event_of`/`enter_of`, one
    non-inlined function per screen: largest Auth 488 B (its path with HOTP
    5,296 B), Now Playing 968 B, lists 648 B. (3) `Fw::auth_step` (4,264 B)
    split: `derive` 4,240 B, `auth_run` 1,232 B, `import_read` apart;
    `sync_player`/`sync_bt`/`sync_rec`/`sync_drive`/`poll_picture` not
    inlined (no change measured; they keep the loop's frame small as they
    grow).
  * The UI future shrank too: `.bss` 124,152 → 123,352 B, so the stack
    (the rest of SRAM0) grows by 800 B to ≈ 25,032 B.
  * Audio (card) task worst path unchanged, **16,980 B**: task closure
    5,832 B (five ≈ 740 B temporaries: many requests' code inlined) →
    `play_file` 2,296 → `Server::serve` 1,976 → `resolve` 792 →
    `Builder::step` 936 → `CardStore::write_at` 660 →
    `orpheus_index::card::Inner::slot` 2,536 → `Volume::open`/`walk` (≈ 1 KB)
    → SD. Next step (not done: RT-level code, resident/hot placement): the
    same non-inlined split for the idle loop's request handlers and for
    `Inner::slot`.
  * Largest frames now: `audio::init_chain` 11,192 (once, before the
    executors), audio task 5,832, `Fw::derive` 4,240, `mount_store` 3,712
    (once), `release` 2,760, `mount` 2,624, Vorbis `setup` 2,576,
    `Inner::slot` 2,536, `play_file` 2,296, `init_player` 2,232,
    `rec::record` 2,216.
  * The measured 2,352 B free is the high-water mark since boot (the
    player had not been rebooted between the earlier USB-6/recorder
    sessions and the suite), not necessarily the suite's.
* **Numbers**: resident 77,916 → 77,924 B (+8: the card-info atomics in
  `.data`), `.bss` 124,152 → 123,352 B, 101 code + 18 rodata pages, slot
  1,060,864 B.
* **Gate**: host fmt, clippy (`--all-targets -D warnings`), `cargo test
  --workspace` (963 passed), `xtask test golden`; fw fmt, clippy; `xtask
  build`, `check-resident`, `size --check`; `cargo xtask dev suite --sim`:
  15 journeys, 48 shots pass (56 expected frames with the variants). Not
  run: the suite on the player, `brand`/`site --check`, `hil smoke`.
* **For the lead**: install the new firmware (`xtask dev stage`), then
  `cargo xtask dev suite --setup` once (uploads the 30 s fixtures), then
  `cargo xtask dev suite`; report the `stack free` line. Expected: all 15
  pass with `auth@damaged`, `home@test-only`, `steps@no-clock` while the
  vault, `/Images` and the clock are as they are now.

### 2026-11-24 — UI-8: list roll-around (worker, no hardware)

* **Toolkit** (`orpheus-ui`): `ListView::handle` takes a `ListCx { now,
  roll }` instead of `now`; screens pass `cx.list()`. `Cx::roll` is filled
  by the shell from the new `System::list_roll_around` (default `true`),
  read once per key, and also drives the menu sheet. A fresh Up/Down rolls
  round past an end, an auto-repeat never does (stops on the end row, also
  at the 5/20-row speeds); Next on the last row → first, Prev on the first
  → last; with the setting off everything clamps as before. The old
  `ListView::wrap` field (unused) is gone. `ListView::step_plain` gives
  screens with their own rows the same rules. The header above a row that
  lands at the top of the view is now kept in view also when focus lands
  exactly on the top row (rolling round in a 6-row settings group hid the
  group title otherwise).
* **Setting**: `id::LIST_ROLL` = `0x030C` (next free Display id), toggle,
  default **on** (the owner asked for it to work), shown between Clock in
  status bar and Home screen; `STORED` 97 → 98; strings `ListRollAround`
  (en "List roll-around", ru "Списки по кругу", ja "リストのループ" —
  "循環" is not in the Unifont subset). Firmware and simulator `System`
  impls read it through `services::list_roll_around`.
* **Screens covered** (all through `ListView`/`ListCx`): menus, Settings
  root and all groups, Choice lists, Home screen apps, Info pages (About,
  card, battery), power menu, quick panel, Files / Music › Folders /
  Pictures / Recordings (windowed: rolling onto the last row loads its
  window), Music root and library lists, Bookmarks, Properties,
  Bluetooth main / found / paired lists, radio presets, alarm list, alarm
  days, Developer rows; through `step_plain`: the Authenticator list (and
  Up/Down in its big-code view) and the stopwatch laps. Unchanged (not
  lists, documented in 05 §5.2): Home grid, timer preset chips and the
  radio's Up/Down preset stepping (always wrap), Steps week chart (seven
  bars, no scrolling), dialog buttons, editors and pickers. **Exception
  for the owner to confirm:** Now Playing's Up next and Details views keep
  changing the view at either end in both settings; rolling round inside
  them would leave Up/Down no way out of the view.
* **Fixes found on the way**: the stopwatch screen let the release of the
  Prev/Next that switched to the Alarm tab page the alarm list (with
  roll-around that jumped to the last row); it now swallows the whole
  press. VER-4 corpus input `58133401…` (a rolled-round Files list into a
  folder whose first name scrolls, then a menu) showed the frozen screen
  under a menu differing from a full redraw because the marquee moved:
  the shell now draws the screen under an open menu/dialog as of the
  moment it opened (`Shell::underlay_at`).
* **Tests**: `orpheus-ui` list tests (both ends with headers, repeat stop
  incl. 1000-row acceleration, paging round and clamped, windowed list
  rolling onto an unloaded row and back, single row/empty, `step_plain`,
  header kept in view), menu sheet with the setting on/off;
  `orpheus-apps` setting test (toggle, default, position, persisted off);
  `sim/tests/roll.rs` walks 31 list screens (every group but Language,
  Files incl. the 5000-entry folder, library, Bluetooth, presets,
  recordings, Authenticator, alarms, alarm days, laps, menu, …) in both
  settings: Up/Down round and back, Prev/Next round, a 20 s held Down ends
  on the last row, and with it off nothing moves past the ends; plus the
  Display row switching it. Existing tests and golden scripts that
  reached the end of a list by pressing past it now use Up (the last row)
  or the right count.
* **Goldens**: new (12) `app_list_roll_{dark_latin,light_cyrillic,dark_cjk}`
  (the row, on), `app_list_roll_off_*` (off), `app_group_rolled_*`
  (Display after Up on the first row: Home screen focused),
  `media_files_rolled_*` (5000-entry folder after Up: Zephyr 192, window
  loaded). Re-blessed (14): `app_display_*` (6) and `app_group02_*` (2):
  the scroll thumb is shorter for the extra row; `media_files_sort_*` (6):
  the track list under the Sort sub-menu is drawn as when the menu opened
  (ellipsis, not a half-scrolled marquee). `docs/shots/app_display_dark_
  latin.png` regenerated. 1,231 goldens.
* **Numbers**: resident 77,428 → 77,444 B (+16, rodata); `.bss` 123,656 →
  123,664 B (+8: the shell's `roll` and `underlay_at`); RAM +24 B (limit
  +64). Paged code 800,394 → 801,522 B (99 pages, unchanged), paged rodata
  137,858 → 137,918 B.
* Gate: host fmt, clippy (`--workspace --all-targets -D warnings`), `cargo
  test --workspace`, `xtask test golden`, `xtask site --check`; fw fmt,
  clippy; `xtask build`, `check-resident` pass. Not run: `size --check`
  (worktree path), `brand --check`, the player (host-only package).
* **User**: check 26 (roll-around on the player, and whether on is the
  right default). Open: the Up next/Details exception above; VER-7's
  on-device journey for roll-around.
| VER-7 | **tooling built (host + `ui top`, `ui test-look`); first full player run 2026-11-25: 5 of 15 pass → every failure triaged and fixed (suite: variants, shot re-reads, masks, 30 s fixtures; firmware: Card info, About hardware, UI stack −6 KB); not yet re-run on the player** | `cargo xtask dev suite`: 15 journeys (`tests/suite/journeys.toml`) in the simulator's script language, run in the simulator (expected frames `tests/suite/expected/`, 48 shots) and on the player (keys, `ui top`, screenshots under `tests/suite/masks.toml`, `cmd:` reply checks), report in `target/orpheus/suite/<ts>/`, `--setup` fixtures in `/.orpheus/test/suite/`, `--sim` dry run, ends in a reboot. 07 §5.7 "As built". Not done: every interaction of every screen (VER-7 proper), audio CRC and card-image oracles |
| VER-8 | **groundwork built, not yet run on the player** | `cargo xtask dev fuzz-ui --seed N --minutes M`: `ui_keys` grammar without Power, a `ui top` guard before every key (dialogs, Bluetooth, USB, Recorder, vault, settings), oracles every K keys (fault record, uptime, stack ≥ `[hil] stack_free_min_bytes`, underruns), card root listing before/after, replay + ddmin into `tests/regress/ui-device/`, `--replay`. Not done: model-guided sequences, injected events, simulator divergence |
| VER-12 | **tooling built (host + 4 firmware test hooks), not yet run on the player** | `cargo xtask dev soak <music\|radio\|browse-while-playing\|settings-churn\|mixed\|stress>`: seeded weighted actions (playback, Now Playing seeks/pause/skips, browsing through the fuzzer's guard, radio, recordings in `/.orpheus/test/rec/soak/`, session-only settings, card hand-over read-only, keyboard personality), oracles every 10 s (fault, uptime, stack, underruns by cause outside seek windows, pager loads/evictions per second, DAC headroom, card/pager errors, DAC and uptime against the host clock), after the run card root, settings store CRC across the reboot, state files; CSV + HTML per run, `metrics/soak/`, findings minimised into `tests/regress/soak/`, `--replay`, `--sim`. Firmware: `settings test-session on`/`test-set`, `ui screen-off`, `status pos=`, STATS words 21–23. 07 §5.10 "As built" |

### 2026-11-24 — VER-7: session-only test look after the first device run (worker, no hardware)

From the lead's first `cargo xtask dev suite --setup`: all 15 journeys
failed at the start ("Home on the player matches none of the simulator's
11 Home frames"): the owner's player uses the green accent, and the suite
must not change the owner's stored settings.

* **Firmware**: `ui test-look on|off` → `test-look on|off`. A RAM flag
  (`ui::TEST_LOOK`, cleared by `off` and by every reboot) under which
  `Fw::setting` returns the defaults for Display 0x0304–0x030C (theme,
  accent, wallpaper, screen saver, animations, rotation, clock in the bar,
  Home apps, roll round), the language and Now Playing's view; the store
  is not written. The UI loop notices the switch (≤ 1 s), reloads Home and
  redraws everything. `ui top` now also reports `look=0|1`.
* **Simulator**: its defaults are this look already (host test
  `simulator_starts_in_the_test_look`); no change.
* **Suite and fuzzer**: `ui test-look on` after `stop`, before the first
  journey / key (an older firmware is reported by name), `off` at the end,
  then the reboot. The Home finder's error now names the closest of the 11
  frames, its differing pixels and their bounding box.
* Re-blessed with UI-8 (roll round): `settings/display` (scroll bar of the
  longer group), `tools/alarm` (first alarm focused) — as the lead did on
  main.
* **Numbers**: resident 77,500 → 77,532 B, `.bss` 123,664 → 123,672 B
  (+8: the flag, aligned), 99 code + 18 rodata pages.
* **Gate**: host fmt, clippy (`--all-targets -D warnings`), `cargo test
  --workspace` (941 passed); fw fmt, clippy; `xtask build`,
  `check-resident`, `size --check` pass; `cargo xtask dev suite --sim`:
  15 journeys, 48 shots pass. Not run: the suite on the player (the lead).

### 2026-11-24 — VER-7 tooling and VER-8 groundwork: on-device suite and UI fuzzer (worker, no hardware)

Host tooling only; nothing was run on the player. Details and how to run:
07 §5.7 "As built".

* **Firmware**: test command `ui top` → `top ID NAME depth=D dialog=0|1
  overlay=0|1 locked=0|1 screen=0|1`, answered at once from two statics the
  UI task writes after every loop (`fw/firmware/src/ui.rs`; +8 B `.bss`).
* **`cargo xtask dev suite [NAME…] [--list] [--update-expected] [--setup]
  [--sim] [--no-reboot]`** (`xtask/src/dev/{script,player,mask,suite}.rs`):
  15 journeys in `tests/suite/journeys.toml` (Home, Settings groups,
  information screens, USB settings, Files, Music, Now Playing over
  fixtures, Pictures, Bluetooth, Radio, Steps, Tools, Recorder,
  Authenticator locked, USB Drive notice), the simulator's script language
  plus `sim:`/`dev:`/`cmd: TEXT ~ REGEX` lines; expected frames from the
  simulator in `tests/suite/expected/` (48); per-screen masks in
  `tests/suite/masks.toml`; Home focus normalised from the screenshot;
  report `target/orpheus/suite/<ts>/` (PNGs, diffs, `summary.txt/json`,
  `index.html`); ends in a reboot.
* **`cargo xtask dev fuzz-ui --seed N --minutes M`** (`fuzz.rs`): `ui_keys`
  grammar without Power, guard on `ui top` before every key, oracles every
  K keys (fault record, uptime, stack ≥ `budgets.toml [hil]
  stack_free_min_bytes` = 2048 (new), underruns), card root listing
  before/after, replay + ddmin into `tests/regress/ui-device/`,
  `--replay FILE`.
* **Host tests** (24 in `xtask` `dev::`): script parsing and the Power /
  simulator-only refusals, `ui top` parsing, key indices against
  `orpheus-input`, masks (inside/outside, tolerances, combination, PNG round
  trip), every journey in the simulator ending at Home, expected frames
  current (`ORPHEUS_BLESS=1` rewrites), the whole suite runner against the
  simulator as the player (all 48 shots exact) and its report, Home focus
  normalisation and the wrong-appearance error, a failing shot in the
  report, the grammar pinned to `sim/fuzz`, no Power in 5,000 generated
  keys, the guard, ddmin, a planted crash found and minimised to Menu,
  Down, OK on a fake player, 3 × 400-key simulator sessions without a
  power/storage/USB/factory-reset request, regressions replay in the
  simulator.
* **Gate**: host fmt, clippy (`--all-targets -D warnings`), `cargo test
  --workspace` (928 passed); fw fmt, clippy; `xtask build`, `xtask
  check-resident` (48 sections from 16 roots, none paged), `xtask size
  --check` pass. Not run: `xtask dev hil smoke` and the new suite (no
  hardware for workers); brand/site checks (no change there).
* **For the lead**: stage a build with `ui top`, then `cargo xtask dev
  suite --setup` and look at `index.html`; expect to tune masks where the
  player's own state shows (settings values, card, vault state, Now
  Playing view). The suite assumes Dark/Blue/English, all Home apps,
  Bluetooth off. Run `fuzz-ui` on a test card first: the guard keeps the
  UI away from destructive actions, but it does change the player's
  volume, alarms, timers and (with `--settings`) settings.
* **Open**: the UsbConnect dialog needs a cable event (not injectable
  yet); the card-write oracle compares only the root listing; VER-7 proper
  (every interaction of every screen, audio CRC, card image diff) and
  VER-8 proper (model-guided sequences, injected events, simulator
  divergence) remain.
### 2026-11-24 — USB-6 device + UI-6 USB Drive: present a disk image to a computer (worker, no hardware)

* **Export rules, host-tested (`orpheus_usb::msc::image`)**: `Medium`
  (CD-ROM for `.iso`, removable disk otherwise, fixed disk), `check()`:
  a CD is always read-only; a disk is read-only unless writing was asked
  for **and** the file is under `/.orpheus/` (`under_orpheus`: first
  component `.orpheus` case-insensitively, no empty/`.`/`..` components)
  **and** test mode; smaller than one block, larger than 2 TiB, or an
  exFAT file with valid length < size are refused. `map_file` maps an
  `orpheus-fs` file into the extent table (offsets checked; > 682 runs:
  too fragmented). `Export::unit()` is the SCSI unit (vendor `Orpheus`,
  product `USB Drive CD-ROM` / `USB Drive disk`). `ImageDevice` now
  refuses writes unless `set_writable(true)` (a second guard behind
  the SCSI write protection); `ExtentTable::from_raw` rebuilds a table
  filled elsewhere (checked: runs in file order, no gap, no overflow);
  `Extent` is `repr(C)`.
* **Host tests** (`crates/orpheus-usb/tests/image_export.rs`): a FAT16
  card with three images written 1 KiB at a time in turn with a filler
  (dozens of runs each), exported over the real Bulk-Only Transport to a
  scripted initiator: every image reads back whole (disk: whole sectors,
  hash equal; CD: padded with zeros); read-only exports refuse WRITE (10)
  with WRITE PROTECTED (disk) / INVALID OPCODE (CD) and the card is
  byte-identical; a writable test image takes a full rewrite, writes past
  it get LBA OUT OF RANGE and change nothing, and **only the image's own
  sectors differ** afterwards (other files and all metadata identical
  through `orpheus-fs`); a too-small table reports too fragmented. In
  `extent.rs`: every addressable block written, then writes past the end,
  overflowing LBAs and odd lengths refused, no card sector outside the
  runs touched, the partial last sector untouched.
* **Firmware** (`fw/firmware/src/drive.rs`, `usbdev.rs`, `audio.rs`,
  `media.rs`): personality `Image` (INFO personality 3). The request
  (app: a `FileRef` of the listing; developer command: a path ≤ 96 B) is
  kept in `drive`; the USB task asks the card task (`media::Req::Image`),
  which stops playback and saves state as for the card reader
  (`hand_over`), opens the file, checks it (`under /.orpheus/` = by path,
  or the file's folder is `/.orpheus/images`), maps its runs into the
  decoder arena's bytes 16–24 KiB (after the card reader's 16 KiB transfer
  buffer; `audio::usb_extents`, same contract as `usb_buffer`) and hands
  the card over with `release` (or answers the refusal and keeps it). The
  USB task serves the unit through `ImageDevice` over the card's DMA
  block driver with the BOT pump of the card reader, publishes counters
  and host presence, and ends on a new personality, `usb dev`, or eject;
  then the card is re-initialised and goes back, the volume is mounted
  again. While the card is out the UI's `card()` is `UsbOwned` (Home
  shows "no card" for the other card apps as before). Developer
  commands: `usb image <path> [cd|disk|fixed] [ro|rw]` (test mode: the
  developer channel), `usb image status` (`image attached host
  medium=cd ro size= runs= read= written=`). Without a mounted card the
  export fails at once (no wait on a card task that is not there).
* **App** (05 §7.10, `screens/drive.rs`, screen ids 42–45): Home row list
  (Presenting · name while one runs, Images, Test images when
  `/.orpheus/images` exists, Choose file…), a folder of images
  (`BrowseMode::DiskImages`: other files dimmed, OK on them "cannot
  open"), Options (name, size, Mode: CD-ROM/Disk/Fixed disk, Read-only
  locked on unless a test image in test mode = developer rows shown,
  Connect), Status (name, mode · access, Connect a computer / Connected,
  Read, rate or Written; Back disconnects, asking first when the computer
  wrote; a refusal says why: too fragmented, too small, not fully written,
  card error, only test images writable). Files › a disk image and the USB
  dialog's "Present disk image…" (also as the "always" choice) open it.
  The `app_not_yet_*` journey now opens a playlist (still not built);
  `media_files_notice_*` a firmware file; new `media_files_disk_image_*`.
* **Simulator** (`sim/src/drive.rs`): the same checks and `map_file` on
  the simulated card, a fake computer (`set:drivehost=1`, 512 KiB/s;
  `set:drivewrite=N`). Card: `/.orpheus/images/test.img` (1 MiB) and
  `scattered.img` (700 one-cluster runs). Behaviour tests
  (`sim/tests/usb_drive.rs`, 9): ISO as read-only CD read back byte for
  byte, test image hash equal, read-only unless test image + test mode,
  the service refuses writable user images / writable CD / too small /
  writable outside test mode by itself, a writable test image changes only
  itself (size kept), read-only refuses the computer's writes, scattered
  image refused with the card kept, Files/USB dialog/Home entry points.
* **Goldens**: 94 new `drive_*` (15 screens × 2 themes × 3 scripts + 4
  amber; looked at all three scripts), 6 new `media_files_disk_image_*`;
  changed: `app_not_yet_*` (6), `media_files_notice_*` (6),
  `recorder_{clip,gain,ready,saved,waiting}_*` (30: "2 min left": the test
  images use card space). Japanese test-pack strings avoid characters the
  Unifont subset lacks. Manual: `docs/user/usb-drive.md` (+3 shots).
* **Numbers** (worktree build): `__stack_bottom` `0x131180` → `0x1314E8`
  (**+872 B** SRAM0): resident 77,428 → 77,820 B (+392: `.data` gets
  `drive::WANT` 104 B, `INFO` 32, `MAPPED` 24 — non-zero initial values —
  and alignment), `.bss` 123,656 → 124,136 B (+480: the USB task's future
  with the image pump, `drive` name/dirs/counter, `Fw` +56). Extent table
  8,184 B in the arena (no new RAM). Paged code 99 → 101 pages, slot
  1,040,384 → 1,056,768 B. `size --check` within budgets.
* Gate: host fmt, clippy `--workspace --all-targets -D warnings`, `cargo
  test --workspace`, `xtask test golden`, `site --check`; fw fmt, clippy
  `--workspace -D warnings`; `xtask build`, `check-resident`, `size
  --check`: pass. Not run: anything on the player (no access).
* **Lead's device run** (one command at a time):
  1. Flash this build (`cargo xtask dev stage …` as usual), wait for Home.
  2. `cargo xtask dev usbimage-test`: uploads `usb6-test.img` (256 KiB +
     300 B) and `usb6-test.iso` (300 KiB + 100 B) to `/.orpheus/images/`,
     presents each read-only (`usb image … disk ro` / `cd ro`), waits for
     the `Orpheus USB Drive` block device (`/dev/sdX`, `/dev/srN`), reads
     it whole, compares SHA-256 with the file (disk: whole sectors; CD:
     zero-padded), prints `usb image status`, detaches (`usb dev`) and
     waits for the device to go. Needs the udev rule (block devices
     0666). Expect two `sha256 equal` lines and `read=` equal to the
     device size.
  3. By hand, the same with a real FAT image and ISO, mounted read-only:
     `truncate -s 8M t.img && mkfs.vfat t.img && mcopy -i t.img README.md ::`;
     `xorriso -as mkisofs -o t.iso -V USB6 docs/user` (or `genisoimage`);
     `cargo xtask dev put t.img /.orpheus/images/t.img` (and `t.iso`);
     `cargo xtask dev cmd "usb image /.orpheus/images/t.img" --wait 5`;
     `lsblk -o NAME,VENDOR,MODEL,SIZE,RO`; `sudo mount -o ro /dev/sdX
     /mnt && ls /mnt && sudo umount /mnt`; `sha256sum /dev/sdX t.img`
     (equal for a 512-multiple image); `cargo xtask dev cmd "usb image
     status"`; `cargo xtask dev cmd "usb dev"`. For the ISO: `usb image
     /.orpheus/images/t.iso`, `isoinfo -d -i /dev/srN` (or `blkid`),
     `sudo mount -o ro /dev/srN /mnt`, then `usb dev`.
  4. Optional: `cargo xtask dev usbimage-test --rw` (writes 64 KiB over
     the test image in `/.orpheus/images/` through the block device, then
     reads it back read-only), and `--qemu --iso <small bootable ISO>
     --banner <text>` (boots it from `/dev/srN` in QEMU with the serial
     console in `target/orpheus/usbimage-qemu.log`).
  5. On the player: Home → USB Drive → Test images → `usb6-test.iso` →
     Connect while plugged in: "Connected", the Read figure grows when the
     computer reads; Back → the card is back (Files lists it). A refusal
     check: Choose file… a large fragmented file if one exists.
  6. User check 26 (a real PC boots from an ISO) when convenient.
* **Open**: the boot-while-holding-Next option and "last used" list of 05
  §7.10 are not built; read speed unmeasured (the card reader does
  2.3 MB/s); whether Windows/macOS boot menus list the CD (MMC answers as
  in the host tests).

### 2026-11-24 — AU-13 fix after the first device run: the microphone recorded silence (worker, no hardware)

From the lead's run of `d7f59d7c`: `rec start mic wav 10` saved
`REC-0001.wav` (442,368 frames, 0 drops, `ring_peak=17196`), but
`level=-60/-60 peak=-60/-60 clip=0` throughout while someone talked;
`rec status` mid-recording: `adc=33001043,4000d2a,180028,103,93,187`,
`dma_events` 151 → 206. The DMA moves, every sample is near zero.

* **Cause (found in the vendor code, [code]): the microphone was routed
  to the wrong side.** The vendor's analogue front-end function
  (`AUD_DEV` B1:0x0D82058C) takes (left, right): left ORs `0xC` into
  `MIC_MIX_CTL` (`[3:2]`, the datasheet's MICL_EN) and `0x70080` into
  `ADC_CTL` (FD buffer L on, 6 kΩ), right ORs `0x3` (MICR_EN) and `0x7040`;
  the mono microphone calls it with (1, 0) together with the left converter
  switches `0x2A` (input code 4). audio.md §3.1 has the two swapped, so
  ours enabled the right mic input (`MIC_MIX_CTL` = `0x103`) and no FD
  buffer (`CTL` = `0x04000D2A`) while capturing the left converter. Now
  VERBATIM: `MIC_MIX_CTL = (v & ~0x13F) | 0x10C` (`| 0x10` single-ended),
  `CTL = (v & 0xFFF8803F) | gain << 8 | 0x70080`. Expected registers:
  `MIC_MIX_CTL` `0x10c`, `CTL` `0x4070daa`. Corrected in 01-hardware §8,
  U-AUD-13 and the PAC (`VENDOR_MIC_LEFT/RIGHT/CLEAR`,
  `VENDOR_CTL_MIC_KEEP`).
* **U-AUD-12 (sample width), not decided by this run.** The vendor's DMA
  mode is `0x40000 | fmt << 20 | 0x8B` with the same `fmt` as
  `FIFOCTL[7]` (B1:0x0D82062C), so `0x0014008B` + `FIFO_DMAWIDTH` = 16-bit
  is what the vendor records with; ours matched it (`FIFOCTL` `0x93`).
  Which half a 16-bit read gets is not in the datasheet, so it is now a
  runtime choice: `rec width 32` uses `FIFO_DMAWIDTH` = 0 and mode
  `0x0004008B` (whole words, sample in `[31:8]`, datasheet §8.2.6.7) and
  narrows each half of the ring in place to bits 31:16
  (`orpheus_rec::pcm::unpack_24in32`; half as many frames per half, so
  23 ms of stereo / 46 ms of mono at 44.1 kHz per half). Default stays 16.
* **Meter**: reads the block the capture hands to the session (the DMA
  half, after low-cut/AGC), not a scratch; checked, unchanged. Host tests
  added for it on the 24-in-32 path.
* **Developer commands**: `rec raw` (while capturing: 16 words from the
  start of the half the DMA is filling, read once it is ≥ 64 B in and
  ≥ 1 KiB from the end, so the reader has not processed them; `ring=`,
  `at=`, `dma_left=` (REMAIN), `settled=`, `fifosta=`, two direct FIFO
  reads `fifo=a,b` (two, so stereo keeps L/R order; the recording loses
  those two samples), `words=`); `rec width [16|32]`, `rec mic
  [diff|se]` (from the next start, not stored; shown in `rec status` as
  `width= mic=`).
* **Code**: PAC `adc` constants and docs; HAL `adc` (`Width`,
  `dma_mode`, the mic front end, `fifo_peek`), `dma::remaining_of`;
  `firmware::rec` (options, `CAP_RING`, the 32-bit unpack in
  `Capture::next`, frames per half from the width); `orpheus-rec::pcm`
  (new, 5 tests: word layout, unpack in place incl. odd length, stereo
  order, a −20 dBFS tone meters −20 through the 24-in-32 path and not
  through the low halves, the chain meters the block it is fed).
* **Numbers**: resident 77,428 B (unchanged); `.bss` 123,648 → 123,656 B
  (+8: `OPTS`, `CAP_RING`); paged code 798,566 → 800,300 B (98 → 99
  pages); slot 1,040,384 B.
* Gate: host fmt, clippy (all targets), `cargo test --workspace`; fw fmt,
  clippy; `xtask build`, `check-resident` pass. `xtask test golden` not
  needed (no screen changed); `size --check` not run (worktree path).
* **Lead's run** (one command at a time, `--wait` for each reply):
  1. `cargo xtask dev cmd "rec status"` → `rec off … width=16 mic=diff`.
  2. `cargo xtask dev cmd "rec start mic wav 20 w16" --wait 3`, then
     while speaking at 30 cm: `cargo xtask dev cmd "rec status"` (expect
     `adc=…,4070daa,180028,10c,93,…` and a level between −40 and −6),
     `cargo xtask dev cmd "rec raw"` twice (words vary, not all 0), wait
     for `rec saved …/w16/REC-….wav`.
  3. `cargo xtask dev cmd "rec width 32"`, then the same with sub-folder
     `w32` (`rec start mic wav 20 w32`, `rec status`, `rec raw`: each word
     should end in `00`; FIFOCTL in `adc=` is `0x13`).
  4. If both are still at −60: `rec mic se`, `rec width 16`, repeat 2 with
     `se16`. End with `rec width 16`, `rec mic diff`, `rec status` →
     `rec off`.
  5. Pull `/.orpheus/test/rec/` with the card reader and listen. Report the
     `rec raw` lines: they decide U-AUD-12 (which half carries the
     sample at width 16).

### 2026-11-24 — WEB-2: Orpheus in the browser (worker, no hardware)

* **`sim/web` (`orpheus-web`, new workspace member, cdylib + rlib,
  `#![deny(unsafe_code)]`)**: the real `orpheus-apps` screens over the
  simulator's `HostServices`, compiled to `wasm32-unknown-unknown` and bound
  with `wasm-bindgen` (no `js-sys`/`web-sys`; the page drives it: wall-clock
  ms in, key edges in, RGBA frames and stereo `f32` PCM out, state as text).
  * **Card**: FAT32 formatted here (`mkfs_fat32`, one sector per cluster,
    ≥ 36 MiB so FAT32 has its 65,525 clusters) and filled through
    `orpheus-fs`: the demo set, `/Your files/…` (names cleaned, duplicates
    numbered), and the library files of the previous visit when the card's
    fingerprint (demo version + the visitor's file names and sizes) matches.
    `orpheus-library` then opens that library instead of building (new:
    `SimLib::build` tries `Library::open` first; the sim's own card has no
    library, so the desktop simulator and goldens are unchanged).
  * **Sound**: the simulator's player decides what plays and where; the
    engine (`audio.rs`) follows it: probe (skipping ID3), `orpheus_codecs`
    inspect/open/seek, `orpheus_dsp::Chain` set from the Sound settings
    exactly as the firmware's `apply_sound`, mono → stereo, `f32` out. A new
    track, pause, or a position that leaves the decoded window restarts the
    decoder there and tells the page to flush. The decode loop is an
    `async` block polled on demand, so it owns the file, arena and decoder
    without `unsafe`. Volume: the firmware's DAC curve (−3 dB at 30,
    1.5 dB/step) as a GainNode. Track lengths are the files' own
    (`Media::duration_probe`): stream frame count or a full decode, cached.
  * **Demo set** (all the project's own; nothing to attribute): four
    tracks synthesised by `synth.rs` (Karplus–Strong lyre with an `.lrc`,
    filtered-noise waves, FM bells, a drum machine) and encoded once by
    `cargo xtask wasm --demo` with ffmpeg into `sim/web/demo/` (MP3 96k
    361 KB, Vorbis q1 295 KB, Opus 48k 248 KB, FLAC 22.05 kHz mono 312 KB,
    Lyra night-sky JPEG 4 KB); WAV test tones (log sweep, left/right),
    BMP/QOI pictures and two texts made at run time.
  * **Recorder/replayer**: key edges → `ui_keys` corpus bytes (taps, holds
    200 + 20·b ms, press/release when keys overlap, waits 37·b + 1 ms; at
    most 48 operations replay); replay decodes with the fuzz target's own
    `command` (included from `sim/fuzz/src/lib.rs`, now `pub`) on a fresh
    session with default settings, as the target starts.
* **Simulator changes (host only, firmware untouched)**: `orpheus-host`
  (USB) is not a dependency on wasm32 (PNG export and the card's PNG are
  cfg-gated); `Sim::with_sys`, `HostServices::with_media`,
  `Media::with_card` (any image, `orpheus_fs::host::Base` = borrowed or
  `Arc` base for `OverlayDisk`), `Media::now_audio/read_file/volume/
  resume_point/set_resume_at`; tags are read with the file's own format
  (FileFormat::tag_format) instead of always MPEG.
* **Page** (`site/pages/try.md`, `site/try/{app.js,worklet.js,try.css}`;
  `xtask site` copies them and keeps `try/pkg`): an SVG drawing of a player
  (not the YM03's exact shape) with pointer/touch keys, hold = long press,
  keyboard map shown and in each key's tooltip; 128 × 128 canvas scaled
  ×1–3 with `image-rendering: pixelated`; the module loads only on this
  page; sound starts at the first key press (browser rule); drop or choose
  files → IndexedDB → the card is rebuilt; settings, volume, resume point
  and library in IndexedDB, saved every 3 s and on hide; Factory reset
  deletes the database; developer panel: frame time, wasm memory, card,
  screen, clock, track/codec/arena, output buffer and dropouts, screenshot
  (native 128 × 128 PNG), key recorder (downloads `ui_keys-*.bin`) and
  replayer. Home, nav and README: "coming" removed.
* **`cargo xtask wasm`** (`xtask/src/wasm.rs`): `cargo build -p
  orpheus-web --lib --target wasm32-unknown-unknown --profile web` (new
  profile: release, opt-level 3, no debug info), `wasm-bindgen --target
  web` **0.2.130** (= the crate in `Cargo.lock`, which nusb's WebUSB back
  end already pins), `wasm-opt -O3` **0.116.1** (Binaryen 116) with the
  target's default features; tools taken from `PATH` only at those
  versions, else `cargo install --locked --root target/orpheus/tools`.
  Gzip sizes against `budgets.toml [web] total_gzip_bytes`; `--record`
  writes `metrics/static.csv` rows (build id `web`; `xtask metrics` keeps
  them); `--test` runs the self-test. `wasm32-unknown-unknown` added to
  `rust-toolchain.toml`.
* **Test** (`tests/web/try.mjs`, Node ≥ 22 or 20 with
  `--experimental-websocket`, no packages): serves the site and
  `sim/golden/`, drives headless Chromium over the DevTools protocol or
  Firefox over WebDriver BiDi. `try.html?selftest` (no stored state):
  Home and Settings (reached by real key events) equal
  `app_home_dark_latin` / `app_settings_dark_latin` pixel for pixel; Files
  › Music › Orpheus demo › Lyre Study by keys; 4 s of playback with no
  dropout; first screen < 2 s; frame budget. Chromium also runs the normal
  page: theme and volume and a chosen file survive a reload, the library
  is restored, developer panel, recorder (3 operations), replay of the
  first committed `ui_keys` corpus input (13 operations, none refused),
  Factory reset. The same screens and the key path are host tests
  (`cargo test -p orpheus-web`, 22 tests).
* **CI**: `pages.yml` builds the module after the site and runs `xtask
  wasm --test` (headless Chrome with `--no-sandbox --disable-audio-output`
  when `CI` is set: the fake sink still runs the audio graph; checked
  locally); `gate.yml` runs `xtask wasm`; both cache
  `target/orpheus/tools` (actions/cache v6.1.0, pinned by SHA). Workflows
  not run here; YAML parsed, actionlint not available.
* **Looked at**: `target/orpheus/try-selftest-{chromium,firefox}.png`
  (Now Playing of Lyre Study with art, 0:04 of 0:30), `try-dev-chromium.png`
  (light theme restored after reload, your file listed, developer panel),
  the page at 390 px (dark) and 1280 px; `sim/web/demo/lyra.jpg`.
* **Not done / open**: Safari (user check 23); a phone; the second
  self-test golden set in light theme; real level meters (the Levels view
  still shows the simulator's synthetic meters; the firmware has no meter
  feed either); album art from tags (the simulator's generated art);
  ReplayGain tags are not read into the chain; gapless joins (the page
  plays the tail of one track and starts the next).
* Gate: fmt, clippy (workspace, all targets; and `orpheus-web` for
  wasm32), `cargo test --workspace`, `xtask site --check`, `xtask wasm
  --test` (Chromium) and the Firefox self-test passed; YAML parsed. Not
  run: device steps (host-only package), CI.
### 2026-11-24 — USB-5 + OTP-2: USB HID keyboard and the Authenticator app (worker, no hardware)

* **`orpheus-hid`** (new host crate, `no_std`, no `unsafe`, 9 tests): the
  boot keyboard report descriptor (HID 1.11 B.1, 63 bytes: 8 modifier bits,
  reserved byte, 5 LED bits out, 6 key slots over usages 0–101; the same
  8 bytes in boot and report protocol), `KeyboardReport`/`Leds`, usage ids;
  the computer's layout (US, UK, German, French) as **one** table
  `char_of(layout, usage, level)` that the typist searches and the fake
  host decodes with, so they cannot disagree; characters behind dead keys
  (German `^` `` ` ``, French `~` `` ` ``) are refused before anything is
  sent; `Typist` (one press and one all-released report per character,
  pause 2–500 ms, default 12), `type_text` over a `Transport` trait (USB
  now; the Bluetooth module in BT-4) — cancel and transport errors end with
  an all-released report; `FakeHost` (what a computer types; counts
  overlapping keys).
* **USB-5 firmware** (`firmware::kbd`, `usbdev`): `Personality::Keyboard` =
  the developer channel's vendor interface + `embassy_usb::class::hid`
  writer (subclass boot, protocol keyboard, 1 interrupt IN endpoint, max
  packet 8, bInterval 4: 4 ms at full speed, 1 ms at high speed). Class
  requests: SET_REPORT (LEDs), boot/report protocol (both accepted), GET_REPORT
  (all released). One job at a time (≤ 64 characters, wiped after); it
  waits for the computer's SET_CONFIGURATION (≤ 8 s, else back to the
  developer personality and the job ends "no computer") plus 300 ms for the
  HID driver; stops on suspend, bus reset and personality change with an
  all-released report, sends one more after a resume. Typing never switches
  away from the card reader (refused). Endpoints: developer 0, card reader
  2 FIFOs, keyboard 1 FIFO (of 4). Developer commands `kbd on|off`, `kbd
  type <text>` (printable ASCII, `\n` = Enter), `kbd status`, `kbd layout
  us|uk|de|fr|setting`, `kbd keypad on|off|setting`, `kbd delay <ms>`, `usb
  kbd`; INFO's personality field is now 0 developer / 1 card reader / 2
  keyboard. Settings › USB: Keyboard layout, Digits from keypad, Enter after
  code (on); the USB dialog's Keyboard and "When connected: Keyboard" select
  the personality.
* **OTP-2 host**: `orpheus-otp` gains a key-only way into the **same**
  sector format: `finish_unlock_key` (Poly1305 streamed over the sector in
  256-byte pieces), `count`/`read_entry` (XChaCha20 seeked to one record),
  `Payload` (the caller's buffer, wiped on drop: push/remove/swap/contains),
  `open`/`save_payload`/`hotp_next_payload`, stepwise `begin_rekey` +
  `finish_create`/`finish_rekey`, `compact_with`, and an entropy `Pool`
  (SHA-256 mixing, HMAC output, ratchet). `service::Authenticator`: state,
  PIN flow with OTP-1's back-off, 60-s relock, import report (a file is
  "consumed" only if every line was taken), provisioning, HOTP shown code,
  delete, move, wipe; the key derivation is a caller-held `Job`. Both paths
  cross-checked (entries written by `Unlocked` read by the key path and
  back, tamper and wrong PIN, chunk sizes 16–4096). `orpheus-otp` 35 tests
  (+14); `orpheus-usb` PROVISION (0x0B) handled (+1 test).
* **Authenticator app** (`orpheus-apps::screens::auth`, `AuthService`,
  `unix_of`): one screen with modes. The honest limit once before set-up
  ("A PIN stops casual use, not someone with a cable."); PIN pad (3 × 4
  keys; Up/Down/Prev/Next move, OK presses, Back deletes or leaves, hold OK
  finishes), set-up with confirmation, unlock with "Wrong PIN · Wait N s"
  (the digits stay during the wait), "Unlocking…"; list: name, code in
  16-px numerals (8-digit codes in the bold UI font), a bar for the seconds
  left (red in the last 5 s), the focused account in the strip, dashes and
  "Clock not set" without a valid RTC; big code (28 px, grouped 3 + 3 / 4 +
  4, bar and seconds; OK types it, Enter after if set); menus Import from
  card, Next code (HOTP), Move up/down, Delete…, Time check (UTC), Change
  PIN, Lock, Wipe vault…, Type the code; provisioning confirmation; import
  report ("File deleted … The card may keep old copies." / "File kept").
  Back locks; 60 s without a key and screen-off lock. Files opens the app on
  `otp-import.txt`/`otpauth.txt`. 67 strings (RU/JA test-pack entries,
  within the committed Unifont subset).
* **OTP-2 firmware** (`firmware::vault`): `NorVault` on `0x7DE000`; only the
  key while unlocked. A key derivation runs to the end in one UI step after
  one frame of "Unlocking…" (clock raised to 144 MHz; ≈ 0.3 s estimated,
  not measured) — its state is on the stack only then. Vault changes borrow
  the first 3,840 bytes of the frame buffer between frames (zeroed after,
  screen redrawn). Import: the card task reads `/.orpheus/otp-import.txt`
  (≤ 8 KiB) into frame-buffer bytes with the picture viewer's slot protocol
  (`media::OtpJob`, `Req::Otp`; the UI draws nothing meanwhile, 5-s
  reclaim), the UI imports and wipes the bytes; if every line was taken the
  card task overwrites the file with zeros and deletes it. Provisioning:
  REQUEST 0x0B → `vault::provision` (only while unlocked) → confirmation on
  the player; host side `cargo xtask dev otp-add 'otpauth://…'`. The key
  and any provisioning request are zeroed before every developer-channel
  reboot and Recovery mode; key presses, battery and RTC feed the entropy
  pool. **KDF device id**: a fixed label `orpheus-ym03` (reading the eFuse
  words needs PMU writes, CLAUDE.md rule 3; changing it later makes vaults
  unreadable). Iterations: `DEFAULT_ITERATIONS` (6000), not calibrated.
* **Simulator**: `sim/src/auth.rs` — the real service on a RAM sector, a
  demo vault (PIN 1234: GitHub, Proton, Bank HOTP 8 digits, Mastodon,
  Госуслуги 60 s, AWS SHA-256 8 digits, 日本), import file, provisioning, a
  fake computer typing through the real typist; `set:vault=…`,
  `set:otpfile=…`, `set:provision=demo`, `set:wrongpin=N`, script check
  `expect-typed:287082↵`. 9 behaviour tests (`sim/tests/auth.rs`: set-up,
  back-off, typing in four layouts and keypad, no computer/clock/HOTP code,
  import, provisioning, delete/move, relock, change PIN/wipe). **190 new
  goldens** (31 journeys × dark/light × Latin/Cyrillic/CJK, 3 amber,
  Settings › USB), 2 re-blessed (`app_group08_*`: the three keyboard rows).
  Looked at: contact sheets of every journey in dark/Latin, light/Cyrillic,
  dark/CJK, light/CJK and the extras; strings shortened until nothing
  important was cut.
* **udev**: `tools/udev/99-orpheus.rules` gains the USB keyboard
  (`0003:1209:0001.*`): hidraw mode 0666 and `LIBINPUT_IGNORE_DEVICE=1`.
  **The owner must reinstall the rules** (`sudo cp tools/udev/99-orpheus.rules
  /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm
  trigger`). With the rule the player cannot type into that computer's
  desktop (the second line is what stops it).
* **RAM** (measured in this worktree, `cargo xtask build`): `__stack_bottom`
  `0x1327C0` → `0x132F60` (**+1,952 B**; budget for the package +1 KiB).
  Static RAM: `.bss` 123,000 → 123,384 (+384: the service core 224 B
  without the derivation state, `Fw` +48, HID class state 36, atomics),
  `.data` +272 (the provisioning slot 192 B — an `Entry` handed from the
  USB task to the UI —, the keyboard job 76 B): **+656 B**. The rest,
  resident rodata +1,336 B, is ~1,184 B of **panic-location file paths**
  of the new source files (`.rodata.str1.1` is linker-generated, so
  resident): 8 workspace paths of ~100 bytes here (~56 in the main
  checkout, so ≈ +1.6 KB there) plus `embassy-usb` hid.rs and `cipher`
  stream.rs. Measured: the resident image holds **14,643 bytes of such
  paths today** (158 files); `--remap-path-prefix` for the workspace and
  `$CARGO_HOME/registry/src/*` in `fw/.cargo/config.toml` would cut that to
  ≈ 4 KB — recommended as a project-wide change, not made here (it changes
  every build and panic record). AppScreen did not grow (the screen caches
  names and codes, not entries). SRAM3: unchanged. Resident code: unchanged
  (`check-resident`: ok, 48 sections from 16 roots, none paged). The key
  derivation and the vault writes run on the stack (≈ 1–1.5 KB at the UI
  task's top level, estimated): check the stack high-water (`info`) on the
  player.
* **Gate** (host): `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` (123 test
  binaries; the first run hit a full disk in `orpheus-fs` powercut, re-run
  clean), `cargo xtask test golden` (blessed, then passing); firmware:
  `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo xtask build` (resident 85,348 B, 94 code + 18 rodata pages),
  `cargo xtask check-resident` (ok). Not run: anything on the player.
* **Not done**: `TextEntry` and manual entry (Base32 keyboard), rename,
  passphrase vaults, iteration calibration, the eFuse device id, drift and
  last-sync in Time check (only UTC now), FIDO; a manual page.
* **Open**: U-USB-11 (interrupt FIFO type 3 with the Actions DMA; one
  8-byte DMA per report, 12 ms apart, the DMA wait times out after 500 ms
  when the host does not poll); whether macOS/Windows accept the combined
  vendor + HID device; the unlock time at 144 MHz; the stack peak during a
  vault write. User check 23.
* **Device test plan (lead; workstation as USB host; reinstall the udev
  rules first so the test keystrokes never reach the desktop)**:
  1. Stage, boot; `cargo xtask dev info` (personality 0). `cargo xtask dev
     cmd "kbd status"` → `mode=other ready=0 … layout=us delay=12`.
  2. `cargo xtask dev cmd "kbd on"`; the player re-attaches (≈ 1 s). Then
     `lsusb -v -d 1209:0001` → two interfaces (vendor FF, HID 03/01/01) and
     endpoint 0x81 Interrupt, 8 bytes, bInterval 4; `dmesg` →
     `hid-generic … input,hidraw…: USB HID v1.10 Keyboard [orpheus YM03
     keyboard]`; `udevadm info /dev/input/eventN | grep LIBINPUT_IGNORE`
     → 1. `cargo xtask dev cmd "kbd status"` → `mode=keyboard ready=1`;
     developer channel commands still work (`info`, `shot`).
  3. Read the reports raw: `sudo cat /dev/hidrawN | xxd` (or
     `hid-recorder`), then `cargo xtask dev cmd "kbd type Hello, 2026!\n"`.
     Expect 26 reports of 8 bytes: press/release pairs (`02 00 0b …` for
     Shift+H, then all zeros), ending all zeros; `kbd status` `typed=13
     last=3` (3 = done). Repeat with `kbd layout de` and type `zy@`:
     usages `1d`/`1c` swapped, `@` as `40 00 14 …` (Right Alt + Q); `kbd
     layout fr` + `kbd keypad on` + `kbd type 123456`: usages 59..5E, no
     modifier. `kbd delay 2` and `kbd delay 500` change the pace. U-USB-11
     is answered when no report is lost or doubled (count them) at delay 2
     and 12; record the result in 01-hardware §9.
  4. Suspend: `kbd delay 500`, `kbd type abcdefghijklmnop`, then suspend
     the workstation's USB port (`echo auto > …/power/control` with a 2-s
     autosuspend, or `systemctl suspend`); after resume `kbd status`
     `last=5` (cancelled) and the last report read is all zeros.
  5. `cargo xtask dev cmd "usb msc ro"`, then `kbd type x` → "the card
     reader is attached"; `usb dev`.
  6. Authenticator: `cargo xtask dev time --set`; Home → Authenticator
     (`keys` or by hand): the PIN note, PIN 1234 twice; `cargo xtask dev
     put <file with the 7 demo lines of sim/src/auth.rs> /.orpheus/otp-import.txt`;
     Menu → Import from card → "File deleted · 7 added"; `cargo xtask dev
     cmd "ls /.orpheus"` shows no `otp-import.txt`. Compare GitHub's code
     with `oathtool --totp -b GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ`. With the
     player in keyboard mode and `/dev/hidrawN` read: OK on GitHub, OK
     again → the 6 digits and Enter (`28`) arrive; the toast says "Typed".
     `cargo xtask dev otp-add 'otpauth://totp/Test:me?secret=JBSWY3DPEHPK3PXP'`
     → the player asks "Add this account?"; Add → 8 entries. Back (locks);
     reopen with a wrong PIN twice → "Wait 2 s"; power key (screen off) →
     locked. `cargo xtask dev stats` / `info`: stack free still ≥ 3 KiB
     after an unlock and an import (the derivation and the vault write run
     on the stack). Time an unlock (log ring timestamps or a stopwatch).
  7. End with `cargo xtask dev cmd "usb dev"` (developer personality) and a
     reboot (rule 4).
### 2026-11-24 — AU-13 + UI-5b: ADC driver, recording, encoders, the Recorder app (worker, no hardware)

* **`orpheus-rec`** (new host crate, `no_std`, no `unsafe`, no allocation;
  27 unit + 7 round-trip tests):
  * encoders behind one interface (`Encoder<B>` over a caller's block
    buffer): WAV 16-bit PCM; IMA ADPCM in WAV (256/512-byte blocks per
    channel, the decoder's own reconstruction so encoder and decoder agree
    bit for bit; `fact` chunk); FLAC levels 0–2 (fixed 1152-frame blocks,
    fixed predictors 0–4 chosen by residual sum in one pass without
    buffers, Rice partitions up to order 3/4/6, L/R·L/S·S/R·M/S at levels 1–2,
    CONSTANT/VERBATIM fallbacks, CRC-8/16, STREAMINFO rewritable, MD5 0);
  * the chain: 100 Hz low-cut, AGC (attack to −12 dBFS, release 3 dB/s,
    ≤ +24 dB, holds below −50 dBFS, gain ramp inside a block, limiter),
    meter (bar falling 24 dB/s, 1.5 s hold, clip at −0.1 dBFS), voice
    activation (−50/−40/−30 dBFS, 2 s hang, before the AGC);
  * `ring::WriteBehind`: the file from its first byte (header included), so
    ring position = file offset; whole-sector writes at sector-aligned
    offsets, a chunk at a time, the piece before the wrap at once, the
    oldest byte at most 1 s old, everything when closing; a write in flight
    is never overwritten;
  * `session::Session<B, S>`: header first, rewritten every 10 s with what
    is on the card (a cut file plays), blocks that do not fit dropped whole
    (newest), counted, a `dropout` cue point; marks as `mark` cue points;
    `cue `/`LIST adtl` at close; pause; time limit; automatic split; card
    full at 1 MiB (+64 KiB allowance); three failed writes stop it;
  * names `/Recordings/REC-YYYYMMDD-HHMMSS.ext`, `FM-101.90-…`, `-2`… on a
    clash, `REC-0001…` without a clock; a fake ADC (tones, noise, bursts).
  * Verified against the project's decoders: PCM and FLAC bit-exact at
    8/16/44.1/48 kHz, mono and stereo, tone/noise/bursts/silence, all
    levels; ADPCM exactly the encoder's reconstruction; FLAC ≤ 70 % of PCM
    on speech-like input; a file cut after a header rewrite plays; a 2-s
    card stall drops whole blocks, one dropout, the file holds the rest.
* **HAL** `atj2157_hal::adc`: microphone (left converter, differential as
  the vendor's config), line-in (AUX0), FM (AUX1); the vendor's API 18
  order VERBATIM (reset, clock, rate, bias, switches, channels with HPF and
  FIFO slot, bias words, FIFO in 16-bit DMA mode, mic front end); gains
  (vendor defaults ± an offset in dB, digital 0.526 dB steps); stop without
  the vendor's `CLKEN1[0]` clear and `CMU+0x80 = 0` (puts the DAC power-up's
  value back), rate function clears bit 4 too. `dma`: channel 6's
  half/complete events counted in its own resident handler.
* **Firmware** `rec.rs`: a mode of the card task (no playback meanwhile;
  the radio keeps playing through its bypass). Arena: 16 KiB capture ring,
  encoder block, one encoded block, the session (written in place), the
  rest a write-behind ring. Capture and writer are two joined futures (the
  capture goes on while the card is slow). State for the UI and `rec
  status` in SRAM3 after the radio's (`rec::SHARED_ADDR`, 116 B, asserted
  below the I/O counter). `/Recordings` is the write prefix only while
  recording or deleting a recording. Auto-off waits while recording;
  power-off stops and saves first. Developer: `rec start <mic|line|fm>
  <wav|adpcm|flac> [secs] [sub-folder]` (44.1 kHz, files in
  `/.orpheus/test/rec[/sub]`), `rec stop`, `rec status` (state, source,
  format, rate, ms, level/peak per channel, clip, written, produced, drops,
  dropped/lost frames, ring fill and peak, marks, file, ADC registers, DMA
  events).
* **UI-5b**: see the table; settings 0x0806–0x080A added (STORED 89 → 94),
  Format lists only what is built.
* **Tests**: `orpheus-rec` 34; `sim/tests/recorder.rs` 10 (arm/disarm,
  record-pause-mark-stop with the file decoded, formats and qualities, FM
  needs the radio and names the station, card stall, VOX, AGC, recordings
  list play/delete, empty list and no card, card full); `sim/tests/apps.rs`
  updated (the Recorder opens). Goldens: 122 new (20 journeys × 2 themes ×
  3 scripts + 2 amber; looked at), 8 changed (`app_not_yet_*` now the
  Authenticator; `app_group07_*`, the Recording group's new rows).
* **RAM** (worktree build): `__stack_bottom` `0x1327C0` (`c91d659f`) →
  `0x132DB0` (**+1,520 B**): resident 83,780 → 85,044 B (+1,264: the new UI
  strings and panic-location paths in `.rodata.str1.1`, `dma`'s ring
  handler), `.bss` +256 B (`rec::REQ` 144, `Fw` +88, `WRITER`, the event
  counter). In the main checkout the eight new `crates/` panic paths are 42 B
  shorter each (≈ −340 B). SRAM3: 116 B at `0x307F80`. The audio task's
  future did not grow. Paged: 87 code pages (was 82), 18 rodata (16).
* Gate: host fmt, clippy, `cargo test --workspace`, `xtask test golden`;
  fw fmt, clippy; `xtask build`, `check-resident` pass. `size --check` not
  run (worktree path length).
* **Not run on the player.** Open: U-AUD-4, U-AUD-11 (`CMU+0x80` while
  playing/FM), U-AUD-12 (which FIFO bits the 16-bit DMA takes), U-AUD-13
  (mic mode and gain), U-AUD-6 (the datasheet's PA→ADC mix bits are a
  candidate loop; not tried). `docs/hw/audio.md` (local, not in the
  repository) is unchanged; its §3 facts are cited in `adc.rs`.
* Lead's run (the reply buffer holds the last reply only, so read each
  with `--wait` before the next command):
  1. `cargo xtask dev cmd "rec status"` → `rec off …`.
  2. `cargo xtask dev cmd "rec start mic wav 10" --wait 13` while speaking →
     `rec saved /.orpheus/test/rec/REC-….wav bytes=≈882k frames=≈441000
     drops=0 … why=time`.
  3. `fm on`, `fm tune <local kHz>` (radio audible), then `cargo xtask dev
     cmd "rec start fm flac 20 fm" --wait 5`, `cargo xtask dev cmd "rec
     status"` twice during it (level moves, `ring` far below `ring_peak`'s
     ceiling, `drops=0`, `lost_frames=0`), listen for clicks or a pitch
     change in the radio (U-AUD-11), after 20 s the `rec saved …/fm/FM-…flac`
     reply; `fm off`.
  4. `rec start mic adpcm` (no limit), `rec stop` after ~15 s → `rec saved
     … why=stop`.
  5. End with `rec status` → `rec off`; pull `/.orpheus/test/rec/` with the
     card reader; `ffprobe` each file (length = recorded time ± 0.1 %, FLAC
     MD5 is 0 = "not computed"), listen. U-AUD-12/13 from the levels. Then
     user check 23 (the app, `/Recordings`).


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

### 2026-11-25 — VER-12: the soak's "DAC headroom fell to -13 frames" with 0 underruns (worker, no hardware)

Finding: 30-minute `mixed` soak, seed 1, 10.4 min, read in `radio-keys 8
2912082481`; `underruns=0` all run; not reproduced on replay
(`tests/regress/soak/mixed-ring-seed1-7.soak`, now committed).

* **What -13 means.** `PcmOut::headroom` is `(written − played) · 1536 +
  fill − at`: frames the DMA has ahead of it. −13: the DMA had fetched 13
  frames of the half the writer was filling beyond the last one it had
  written: 13 frames (0.27 ms at 48 kHz) of the ring's previous lap (64 ms
  old PCM) went to the DAC instead, at least (the loop looked only now and
  then, so the deepest point may have been lower). The writer then caught up
  inside the half, which the underrun counter could not see.
* **Why `underruns` stayed 0 (bug, fixed).** Underruns were only counted at
  the writer's half boundaries, as halves the DMA had *finished* beyond the
  writer (`played > written` after the increment). A DMA that overtook the
  writer inside a half, or finished the writer's half but not the next one,
  played stale frames with nothing counted. In a model of the ring (one
  tick per DAC frame, every ring frame stamped) the old rule counts **0 for
  any stall that leaves the DMA up to one half past the writer: stalls of
  46–87 ms played 1–3 halves with stale frames and counted none**; longer
  ones came 2 halves short. Fix: `orpheus_audio::pcm::Book` (host crate):
  every look at the DMA counts the halves it fetched stale frames from, once
  each (`stale_halves_are_counted_exactly_for_any_stall` checks the count
  against the stamps for stalls up to 9,000 frames); the firmware looks
  before every block's pushes (where a stall leaves the writer furthest
  behind), at every half boundary and in the loop's chore checks. The
  ledger's causes still add up to `underruns`; the too-slow abort keeps
  counting whole halves only (unchanged behaviour). `regress_overrun_inside_half_uncounted`.
* **A measurement race (bug, fixed).** `headroom` read the DMA's position,
  then the half count: a half interrupt between the two reads (DAC DMA at
  `Level::Deadline` preempts the RT executor) made the reading exactly one
  half too low; with 1500 frames in the writer's half and the DMA 23 frames
  before the crossing it gives exactly −13 for a true 1523. Now
  `orpheus_audio::pcm::read`: count, position, count again, retried until
  the counts agree. `regress_headroom_irq_between_reads`.
* **Which one it was: not settled without the player; ranked.** (1) A real
  shallow overrun: the samples before it in `browse pictures` (48 kHz,
  CPU 0.1–0.6 % idle, 3,000 line loads/s) read low-waters of 227, 315 and
  823 frames, and the −13 sample covered the **76 s since the previous
  sample** (550.7 → 626.9 s; the soak took no sample in between), i.e. the
  end of `browse pictures`, the way into Radio and the start of
  `radio-keys`, with music playing until the radio stopped it. (2) The race:
  needs the half interrupt inside a few instructions; the 102 samples of the
  earlier run (055404) never read below 1200. Both are fixed; the new log
  line tells them apart (a race cannot produce it any more).
* **The oracle (not the cause).** Radio transitions cannot dip the
  headroom: the radio's start stops the music (`PlayReq::Stop`, the DAC
  aborted, `headroom` = none) before the bypass is switched, and every
  stream (re)start fills two halves first; pause and seek stop the DAC too.
  So a dip while music still plays is steady playback and is right to
  flag; exempting `radio-keys` while it runs would hide it. What was wrong:
  the low-water covers the whole time since the previous sample, which here
  began in the previous action, and the finding did not say so (it read as
  a dip "in radio-keys"; the action started at most 76 s before, not 5.5
  min). Findings now end with `; over the 76.2 s since the sample in action
  #5` (ring and underrun). `regress_ring_finding_names_the_span_it_covers`.
* **Instrumentation.** Log line, at most once a second, when a look finds
  negative headroom: `pcm: head -13 blk w=… f=… p=… at=… n=… dec=…ms mk=…
  ub=serve:41ms +3` — the look (`blk` before a block, `half` at a boundary,
  `loop` a chore check), writer half and fill, DMA half and frame, halves
  newly counted, the last decoder call, the loop breadcrumb, the longest
  operation since the last half boundary (the candidate cause) and the looks
  not logged since the previous line. `hot.toml`: the look's functions in
  `play-block`. Resident 79,364 B (unchanged), slot 1,093,632 B.
* **On the player (lead):** replay `cargo xtask dev soak --replay
  tests/regress/soak/mixed-ring-seed1-7.soak` and a 30-minute `mixed` run;
  read the log ring after any `ring` or `underrun` finding: a `pcm: head`
  line names where and after what the DAC ran past the writer. Expect
  `underruns` to be non-zero now wherever the headroom went negative (the
  underrun oracle fires with the ring oracle). If `ub=` names `fill` or
  `serve` with tens of ms the card work in the loop is the cause (the 4-ms
  rests of `66f1b467` double card time); `dec=` tens of ms with `ub=-`
  means decoding under pager pressure from the Pictures UI.
* Gate: host fmt, clippy `-D warnings`, `cargo test --workspace` (debuginfo
  off: the disk is full); fw fmt, clippy, `xtask build`, `check-resident`,
  `size --check`: pass. Not run: `xtask dev …` (no player), brand/site.

### 2026-11-25 — soak seed 1 on the current build (lead)

* `dev soak mixed --minutes 30 --seed 1`: no reset, no fault; stopped
  at 16.9 min on (a) the known Pictures-while-playing starvation (a
  screenshot chunk unanswered 2 s; 1–2 underruns from `serve` during the
  finding's replays) and (b) "uptime +1,613 ppm against the host clock" —
  **an oracle artefact**: the sample was stamped before the fault-record
  and uptime requests, and the starved player answered ~1.6 s late. Fix:
  the uptime is stamped at the middle of its own request and replies
  slower than 200 ms are left out of the clock oracle
  (`regress_late_reply_is_not_clock_drift`). Stack free ≥ 6,392 B, line
  loads/s mean 1,736, headroom ≥ 931 frames over the run.
* The Pictures-while-playing starvation waits for the T-8 profiler
  (worker running).

### 2026-11-25 — plan: setting → effect coverage (VER-13, owner's request)

* M11 gains VER-13 (07 §5.12): every setting × value × context proven by a
  differential test with an oracle (simulator for UI effects, the player
  for sound, clock, timers, USB, Bluetooth), global settings in every
  context, local ones in theirs, no leaks elsewhere, pairwise for
  interacting settings, persistence/defaults/clamping/migration;
  `xtask settings-matrix --check` gate at 100 %. CLAUDE.md: a new setting
  lands with its manifest entry and tests.

### 2026-11-25 — honest underrun count exposed paced refills; fixed (lead)

* With `6db03fb0` (headroom read race fixed, underruns inside a half
  counted), the seed-1 ring finding replayed as **27 real underruns** while
  browsing Pictures during playback; the new `pcm: head` lines said why:
  `ub=fill:25–29ms` — the card rests (`66f1b467`) slowed the playback
  ring's refill until refill + decode (8–9 ms) no longer fitted a 32 ms
  DAC half. Fix: `yoton_bsp::card::Urgent` around the refill; the rest
  decision moved to `orpheus_power::pace::should_rest`
  (`regress_playback_refill_is_never_paced`).
* Replay on the player: **no underruns, no negative headroom, no reset**.
* **Open (performance, M13/PWR-7/VER-10)**: during that Pictures step with
  music playing the CPU is ~0 % idle at ~3,000 line loads/s and thread mode
  is starved for seconds — a developer-channel request (screenshot chunk)
  went 2 s unanswered (USB data-timeout; the player stays up). The cover
  search reads ~30 entries per 250 ms with ~900 line loads: paging, not the
  card, dominates. Needs a paging profile of "Pictures while playing".

### 2026-11-25 — plan: M17 synth engine, MIDI and Sequencer (owner's request)

* New milestone M17 (08; spec 04 §9 and 05 §7.16; PLAN item 16):
  `orpheus-synth` (PolyBLEP oscillators, FM, samples, drum models, SVF,
  envelopes, LFOs, 16-voice target, sample-accurate events, effects,
  limiter; bit-identical host/player renders), MIDI file playback (SMF 0/1,
  own GM sound set, SF2 stretch), Sequencer app (16 patterns × 8 drum + 2
  synth tracks, steps with velocity/probability/ratchet/locks, songs,
  WAV/MIDI export, optional USB MIDI clock with M16). WPs SYN-1…3, MIDI-1,
  SEQ-1/2. 04 §9 Work packages renumbered to §10.

### 2026-11-25 — first full soak pass on the player (lead)

* `dev soak mixed --minutes 30 --seed 2` (before the headroom fix):
  **PASS**, 36.4 min, 18 actions (playdir, menus, radio seek and keys,
  library and Pictures browsing, long listens, idle, pause, DSP,
  Now Playing views), 190 samples: stack free ≥ 5,840 B, line loads/s mean
  2,469 / max 3,303, evictions/s max 3,088, 0 underruns, card errors 0,
  uptime drift −22 ppm, CPU idle mean 29 %. Headroom low-water −960 frames
  inside a disruption window (counted as underruns from `604ffc41` on).
* Seed 1 before that: past the old reset; stopped on the ring finding
  (`mixed-ring-seed1-7.soak`), fixed by the merged `6db03fb0` (headroom
  read race; underruns inside a half now counted; `pcm: head` log line).
* Merged `6db03fb0`; resident 79,364 B; staged; replaying the ring finding.

### 2026-11-25 — plan: M16 USB sound card (owner's request)

* New milestone M16 (08; spec 06 §2.7; PLAN item 15): the player as a USB
  audio interface — UAC2 (UAC1 option) composite with the developer channel
  and HID consumer control; host playback to the jack and Bluetooth,
  microphone (or radio/loopback) to the host; asynchronous with feedback
  from the DAC position against SOF counts; volume/mute both ways; meters,
  route and monitor on a sound-card screen; safety notice for the jack
  with USB connected. WPs UAC-0…5 (UAC-0 proves isochronous endpoints and
  full duplex first; new unknowns U-USB-15, U-AUD-14). Supersedes USB-7.

### 2026-11-25 — plan: factory-firmware reference player (owner)

* The owner has a second, identical YM03 on the factory firmware for
  side-by-side comparisons over Bluetooth and the line input. Planned in
  07 §5.11 and M13 (MEAS-1, AQ-1, PWR-6): same files under
  `ORPHEUS-REF/` on its card, every audio figure and AU-0b checked on both,
  its runtimes as the battery baseline. New hard rule 9 in CLAUDE.md: no
  Orpheus tool ever talks to it; never on USB while `xtask dev` runs.

### 2026-11-25 — soak reset found: card loops never yielded (lead)

* With the surviving log and the new post-reset PC report, the soak's
  saved finding `tests/regress/soak/mixed-reset-seed1-4.soak` (browse
  Files → a track opens; then Pictures) showed the RT executor stuck in
  `pictures::find_cover` (album art for the opening track: a folder scan
  of up to 4,096 entries) — the screen being Pictures was incidental.
* Cause, general: the SD driver spins for its microsecond transfers, so a
  loop of card reads on the RT executor never yields; thread mode (UI,
  USB, the watchdog task) starves and a long loop ends in a watchdog
  reset (also behind the Shuffle-all reset and the old card-walk fix).
* Fix: the card's read/write rest 4 ms after every 8 ms of card work on
  the RT executor (`orpheus_power::pace`, `regress_card_work_rests_every_8_ms`);
  not inside synchronous pollers (`orpheus_fs::SyncPoll`, set by
  `orpheus_index::card::spin`: a first version slept there, a timer that
  never fires, and hung — `regress_spin_marks_a_synchronous_poller`);
  thread-mode users (USB card reader) unpaced. The cover search is capped
  at 250 ms and logs its cost.
* Replay on the player: **no reset any more**. Measured: the cover search
  read **37 entries in 253 ms (~7 ms per entry)** — directory reads are
  far too slow there (open; host investigation next). A screenshot
  request during the Pictures step still times out once (USB data-timeout,
  the player stays up) — open.

### 2026-11-25 — a start-up fault in slot A caught by the fallback; check-resident hole closed (lead)

* Chasing the Pictures stall (soak seed 1, step 5 `browse pictures`: the
  UI and USB blocked ≥ 2 s, once past the watchdog) needed the log lines
  from before a reset, so `log::init` now keeps an intact ring and appends
  `--- boot ---` (`logring::keep`, `regress_log_ring_survives_a_reset`).
* That build **faulted at every start** (fault record: PC 0x2276fc =
  paged `atj2157_hal::log::line`, called by `log::init` before the pager
  started). **Stage 2 fell back to slot B** after three failed starts —
  the T-6 fallback working on a real failure — so the player kept running
  (yesterday's build) and the replays silently tested the old firmware.
* Why the gate missed it: `log::init` and `pager::arm_deny` were inlined
  into `main`, their `resident.toml` roots matched nothing, and
  `check-resident` only warned. Now an unmatched root fails the check
  (`regress_unmatched_resident_root_is_an_error`) and both functions are
  `#[inline(never)]`: 52 sections from 18 roots, none paged; resident
  79,332 B (+136 B for `log::line` and friends).
* On the player: slot A runs the new build; after `hang` (watchdog reset)
  the ring shows the previous start's lines, `--- boot ---`, then the new
  start.
* D-14 decided: English only for speech (TTS-3 dropped).
* Pictures stall: not reproduced by hand with Shuffle all playing; the
  failing soak runs had reached Shuffle all through the (now fixed)
  roll-around navigation. Open.
* UI finding: the now-playing strip's scrolling title looks smeared on the
  player (screenshot during Pictures with a track playing). Open.
* Follow-up: the soak should check the running build id against the one it
  staged (a slot fallback otherwise goes unnoticed).

### 2026-11-25 — plan: M14 games, M15 speech, accessibility and books (owner's request)

* M14 (08; spec 05 §7.13; PLAN item 13): Games app with Blocks,
  Solitaire (Klondike), 4096 and an artillery duel (working name "Salvo":
  generated terrain, gravity presets, wind with gusts acting through
  quadratic drag, computer players that aim by simulation, pass-the-player
  turns), settings per game, saved games, scores, deterministic replays and
  invariant fuzzing. Original implementations of the genres. WPs GAME-0…5.
* M15 (08; spec 04 §8 and 05 §7.14–7.15; PLAN item 14): `orpheus-tts`
  (normaliser, public-domain English rules + permissive lexicon, own
  formant synthesiser, ≤ 64 KiB, mixed over music), screen reader and
  vision/motor settings, Books app (TXT, Markdown, FB2, EPUB) with
  read-aloud. WPs TTS-1…3, A11Y-1/2, BOOK-1…3. New decision D-14 (speech
  languages). Both come after M10, before M13 and M11; the v1 non-goals in
  05 §8 updated.

### 2026-11-25 — plan: M13 unplugged measurement and optimisation (owner's request)

* New milestone M13 (08; spec 07 §5.11; PLAN item 12): audio quality,
  battery, power and performance measured with USB disconnected — output
  into the workstation's input, battery discharging, a firmware run engine
  with audio markers and card logging, an optional Bluetooth status channel
  (0x31 metadata via BlueZ) — then optimised one change at a time. WPs
  MEAS-1…4, AQ-1/2, PWR-6/7, PERF-1; PWR-4 superseded. The jack rule in 07
  §7 now allows the workstation input while USB is disconnected; an armed
  run powers the PA and virtual ground down if VBUS appears. New user
  decisions D-12 (rig: manual swaps by default; isolation transformer;
  `uhubctl` hub) and D-13 (discharge below 3.55 V).
* Also: owner's policy that every fixed bug gets a `regress_` test
  (CLAUDE.md, 07 §3.1); backfill worker running.

### 2026-11-25 — fix: Music › Shuffle all reset the player (lead, found by the soak)

* Third soak (seed 1): a watchdog reset (no fault record) reproduced and
  minimised by the tool. Cause found by hand on the player: **Music ›
  Shuffle all** sampled the whole library's track table (8 rows per card
  read) in one synchronous library session on the RT executor, holding the
  CPU past the 5.6-s watchdog on the owner's library. The soak reached it
  because UI-8's roll-around made its "hold Up to Music's first row (Now
  Playing)" wrap and land on Shuffle all.
* Fix: `Library::sample_begin`/`sample_step` (resumable selection
  sampling; `sample` is now a wrapper; host test: the same picks for steps
  of 1, 8, 13, 64 and 1,000 rows, and a stopping sink ends it); the
  firmware samples 256 rows per session and sleeps as long as each step
  took (≥ 2 ms), as the card walk already does. On the player: Shuffle all
  → playing a 256-track shuffled queue, 0 underruns, no reset.
* Soak: the session turns list roll-around off before navigating to Now
  Playing (again after a `look` action that changed it), and skips that
  navigation without a test session. Simulator `mixed` seed 1: PASS.

### 2026-11-25 — fix: a stale test request survived warm reboots (lead)

* The second 30-min soak (after USB-W1) ended at minute 5 with the player
  in USB recovery: log "fw: test window over". Cause: the boot mailbox's
  test request (`request` = Test, `test_mode` = YTST, from an earlier
  `xtask dev ram --test fw --arg 300`) survives warm reboots and nothing
  cleared it, so every later start from flash ran as a 300-s test and
  quit to USB recovery (also why the stage 2 crash test detoured through
  USB recovery). Fix: `mailbox::stale_test` (host-tested) — a start that
  stage 2 recorded as a slot start from flash is never a test;
  `persist::forget_stale_test` clears the words before anything reads them
  and logs "fw: stale test request cleared". `dev ram` writes the whole
  mailbox (boot info 0), so real tests are unaffected. Resident +96 B
  (79,148 B). On the player: test-mode word 0 after a normal start.

### 2026-11-25 — USB-W1 fix verified on the player (lead)

* Merged `c7b46d7c` (drop-safe EP0 SETUP read, abandoned-transfer
  handling, controller re-init on VBUS loss, liveness-driven recovery,
  guarded watchdog, USB counters). Resident 79,052 B; bss 123,988 B.
* `cargo xtask dev usb-cancel --rounds 3` (the incident's sequence: a TEST
  OUT held 2.5 s at setup, data and status stage, each followed at once by a
  PEEK, plus 40 transfers cut after 1 ms per round): **PASS**, INFO answered
  after every provoked failure (worst 0.0 s), 7 transfers given up, 10
  `CX_COMABT` seen and cleared (so the bit is real on this chip, U-USB-14),
  0 recoveries needed, 0 stalls.
* `usb recover`: the player re-enumerated within seconds (re-inits 1,
  inits 2).

### 2026-11-24 — first soak: USB wedges during use (lead) — open bug USB-W1

* Merged VER-12 (soak tooling). First run `dev soak mixed --minutes 30
  --setup --seed 1`: at minute 3.2, on a `volume` action, the developer
  channel failed (`control OUT 0x6: Cancelled`), came back after 1.5 s,
  then `control IN 0xa` (PEEK) stalled and the player **stopped answering
  USB entirely** — even after re-plugging, on two ports, the kernel logged
  "device not responding to setup address … error -71". The UI stayed
  responsive and the watchdog did not fire (it watches the executors, not
  USB). The owner's reset did not reach the player; the power menu's
  Recovery mode rebooted it, and a normal boot then enumerated fine (no
  persistent cause). **Bug USB-W1**: the USB device stack can wedge for good
  at run time; it needs a root cause, self-recovery (re-initialise the
  controller when setup packets go unanswered) and USB liveness in the
  watchdog health check. The soak cannot run unattended until then.

### 2026-11-24 — stage 2 slot fallback installed and verified (lead, owner's approval)

* Merged T-6 (stage2) `699e2cf4`: stage 2 falls back to the next startable
  slot (also a superseded one, after a trip) when a slot fails 3 starts;
  964 of 1,024 B. Installed with `install-stage2` (old one saved in
  `local/stage2-previous.bin`) after putting the current firmware in both
  slots (`stage` A, `update` B, `switch-slot` A).
* **Crash test on the player**: `boot crash-test` + reset → slot A failed
  3 starts → **stage 2 started slot B** (`boot status`: "slot B, start 1,
  after A failed"; `dev slots`: "fallback: A failed 3 starts"); reset after
  the good-boot confirmation → slot A again. The crashes went through USB
  recovery mode between starts because a test-mode marker from earlier
  test windows was still in RAM (fault reboots return to ADFU only in test
  mode); the counter survived those detours. In normal use a crash reboots
  straight into the next start.

### 2026-11-24 — VER-7: the on-device suite passes on the player (lead)

* Merged the triage (`437c2d1c` UI-2: Card info and the About hardware
  string; `6efb4131` VER-7: retries for marquee timing, the full layout,
  30-s fixtures, journey variants for player state, the UI task's stack
  path cut from 13,640 to 7,608 B). After a fresh boot:
  `cargo xtask dev suite --setup` → **15 journeys, 48 shots, 0 failed**
  (`auth@damaged`, `home@test-only`, `steps@no-clock`), **stack free ≥ 6,136
  of 23,768 B** over the run (was 2,352 B before the stack work).
* For the owner: the Authenticator vault sector still holds bytes from
  before the full-layout switch and reads as "Vault damaged"; Menu › Wipe
  vault… there loses nothing.

### 2026-11-24 — plan: long soaks (VER-12, owner's request); UPD-1/WEB-3 merged

* Added VER-12 / 07 §5.10: several long soak scenarios with interactivity,
  listening to music and the radio, seeking, settings changes and more
  listening, with oracles for stack exhaustion, buffers and paging,
  time-series metrics and regression sequences. PLAN.md item 11 extended.
* Merged UPD-1 (`.ofw` update files, Settings › System › Update firmware,
  developer-channel `UPDATE` request, `xtask dev update/slots/switch-slot`)
  and WEB-3 (browser installer over WebUSB, Install page). On the player:
  the build stages and boots; the developer interface carries the WebUSB
  descriptor (landing page `…/orpheus/install.html`) and an MS OS 2.0
  descriptor; `dev slots` reads A valid / B empty. **The update path was approved by the owner
  and added to CLAUDE.md rule 1.** On the player: `dev update
  orpheus-0.1.0+cf5f4240.ofw` wrote and verified 1,085,440 B into slot B in
  31.7 s, the player restarted from slot B and played (0 underruns);
  `dev slots` showed A superseded / B valid; `dev switch-slot` + reset went
  back to slot A (both valid). Stage 2 does not
  yet fall back to the other slot when a valid image keeps crashing
  (install-stage2 step; open).

### 2026-11-24 — USB-6: disk and CD images served to a computer (lead)

* Merged USB-6 device + USB Drive app (`d6342f09`); fixed a service-kind
  collision from an earlier merge (RECORDER 8, AUTH 6, USB_DRIVE 7).
* `xtask dev usbimage-test` on the player: `usb6-test.img` as a read-only
  disk → `/dev/sdc`, 262,144 B read in 0.36 s, **sha256 equal**;
  `usb6-test.iso` as CD-ROM → `/dev/sr0`, 309,248 B in 0.17 s, **sha256
  equal**; 0 bytes written. Test-tool fixes: device nodes are opened with a
  retry while udev applies permissions, and a transfer error on a
  personality switch is only a note (the player re-enumerates mid-request).

### 2026-11-24 — UI-8 list roll-around on the player (lead)

* Merged UI-8 (`9d9cdfa6`). On the player: Settings root, Up on the first
  row (Sound) → last row (System), Down → Sound again. Open question for
  the owner: Now Playing's Up next / Details views switch views at their
  ends instead of rolling round (rolling would leave no way out by
  Up/Down).

### 2026-11-24 — plan: list roll-around (owner's request)

* Added UI-8: every scrollable list rolls round at both ends (Down on the
  last row → first, Up on the first → last; Prev/Next pages too), owned by
  `ListView` so all lists behave the same; auto-repeat stops at the end;
  Settings › Display › List roll-around (on/off). 05 §3, §5.2, §7.11 and 08
  updated.

### 2026-11-24 — the recorder records from the microphone (lead)

* Merged the AU-13 mic fix (`596ee066`: the left mic input and feedback
  buffer per the vendor code; `audio.md` §3.1 had left/right swapped). On
  the player: `rec start mic wav 20` → **level −27…−30 dB, peaks −22 dB** in
  a quiet room (was −60 = silence); `ADC_CTL` 0x4070DAA, `MIC_MIX_CTL`
  0x10C. **U-AUD-12 answered**: a direct FIFO read gives the 24-bit sample
  in bits 31:8 (`fc608000`), and the 16-bit DMA words carry the matching
  top 16 bits (two samples per word, first in the low half). Listening to
  the files is user check 25.

### 2026-11-24 — USB keyboard works; recorder captures silence (lead)

* Merged AU-13/UI-5b (recorder) and USB-5/OTP-2 (USB keyboard,
  authenticator); firmware builds now remap source paths (resident
  77,428 B with both merged, was 79.4 KB before them).
* **USB keyboard on the player**: `kbd on` → interfaces vendor + HID boot
  keyboard (03/01/01, EP 0x81 interrupt IN, 8 B, bInterval 4); the desktop
  ignores it through the project udev rule (`LIBINPUT_IGNORE_DEVICE=1`);
  `kbd type Hello, 2026!` → 24 reports read on hidraw, decoded exactly
  `Hello, 2026!` (U-USB-11: interrupt IN works). Authenticator on the device
  not yet exercised (user check 24).
* **Recorder**: files are written correctly (`rec start mic wav 10` →
  442,368 frames, 0 drops), but every sample is silent (`level=-60/-60`
  while speaking; DMA events advance, ADC registers configured). Suspect
  the FIFO sample layout (U-AUD-12) or the mic path (U-AUD-13); a worker is
  adding `rec raw` and fixing it.

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
| 2026-11-24 | Stage 2 with slot fallback (T-6) | 964 B of 1,024 (first cut 1,004 B) | `cargo build --release -p stage2`, `__stage2_end` |
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
| 2026-11-24 | Firmware with `ui top` (VER-7), worker worktree build | resident 77,500 B (`size --check`: 77,495), bss 123,664 B (+8: `TOP`, `TOP_NAME`), 99 code + 18 rodata pages, slot 1,040,384 B | `xtask build`, `xtask size --check` |
| 2026-11-24 | Firmware with `ui test-look` (VER-7), worker worktree build | resident 77,532 B (was 77,500), bss 123,672 B (was 123,664: +8, `TEST_LOOK` aligned), 99 code + 18 rodata pages | `xtask build`, `xtask size --check` |
| 2026-11-25 | Firmware after the VER-7 triage (Card info, UI stack), worker worktree build | resident 77,924 B (was 77,916; `size --check` 77,919), bss 123,352 B (was 124,152: the UI future without the store mount), 101 code + 18 rodata pages, slot 1,060,864 B | `xtask build`, `xtask size --check` |
| 2026-11-25 | Static worst stack paths (direct calls; start-up and panic paths left out) | UI task 13,640 → 7,608 B; audio task (RT executor) 16,980 B unchanged; `AppScreen::event` 3,448 B → one frame per screen (Now Playing 968, lists 648, Authenticator 488 B); UI poll frame 4,816 → 904 B | `llvm-objdump` call graph (worker script) |
| 2026-11-24 | Firmware with the VER-12 soak hooks, worker worktree build | resident 78,703 B (`size --check`; was 78,671), bss 124,660 B (was 124,648), `__stack_bottom` 0x131a40 → 0x131a70 (stack −48 B), 104 code + 18 rodata pages, slot 1,085,440 B (unchanged) | `xtask build`, `xtask size --check`, `llvm-nm` |
| 2026-11-24 | Soak tool in the simulator (VER-12) | `mixed` 30 min of simulator time: 24 actions, 197 samples, ≈ 16 s on the host; 14 host tests 0.6 s | `xtask dev soak mixed --sim`, `cargo test -p xtask soak` |
| 2026-11-24 | On-device suite estimate (VER-7) | 15 journeys, 48 shots, ≈ 106 s + reboot; expected frames 256 KiB | `cargo test -p xtask list_estimates`, `du` |
| 2026-11-24 | Firmware with SENS-1, UI-5c, UI-5d, worker worktree build | resident 83,316 → 83,484 B; `__stack_bottom` 0x132238 → 0x132498 (+608 B); 75 → 79 code pages (+16 rodata), slot 835,584 → 868,352 B; cluster `steps-sample` 3,648 B (4 lines) at 0x222000; sensor task poll frame 68 B, audio task poll 4,992 B (unchanged) | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |
| 2026-11-24 | Step detector on synthetic traces (25 Hz, 8 seeds × walk/slow/brisk/pocket 120 s, run 90 s) | all within ± 5 %; 0 steps: table 10 min, handling, bus 10 min, shaking 3.8–8 Hz | `cargo test -p orpheus-steps` |
| 2026-11-24 | ASan host tests | 63 binaries, 586 tests, 0 memory errors; LSan: 4 binaries (1 fixed) | `xtask test asan --lean` |
| 2026-11-24 | Miri | 14 crates, 232 tests, no UB; full run ≈ 35 min | `xtask test miri` |
| 2026-11-24 | Browser build (WEB-2) | module 2,677,523 B (1,816,547 gzip; 1,219,820 B of it the committed demo files), glue 19,664 B, page scripts and styles ≈ 26 KB; 1,830 KB gzip in all (budget 4,000 KB; also under it uncompressed) | `xtask wasm` |
| 2026-11-24 | Browser build in headless Chromium 155 / Firefox 157 (localhost) | first screen 155–220 ms; per-frame simulation + draw 0.08–0.11 ms mean, ≤ 1 ms worst; 230 frames in 4 s; 4.0 s of Lyre Study played with 0 dropouts (also with Chromium's fake sink); wasm memory 80 MiB with the 36 MiB card | `xtask wasm --test`, `tests/web/try.mjs --browser firefox` |
| 2026-11-25 | Firmware with USB-W1 (drop-safe EP0, liveness, counters), worker worktree build | resident 79,052 B (`size --check` 79,043), `.bss` 123,348 → 123,476 B (+128: watchdog task pool 144 B with the detector, 52 B of USB counters/flags), `.data` 16,340 B unchanged, 105 code + 18 rodata pages, slot 1,093,632 B | `xtask build`, `xtask size --check`, `cargo build --release -p firmware` + `llvm-size` of `e91558eb` and USB-W1, `llvm-nm` |
| 2026-11-25 | Index lookup of a listing (`job:index`), owner-like images (exFAT 128 KiB / FAT32 512 B, root 300–1,000 folders, 40–100 files per shard), cold sectors | walk cache as on the player 6–15 sector reads; after an index build, before the fix 102–334 reads / 1,582–5,121 slots (≈ 56–182 ms at 0.5 ms a read), after it 5–14 reads; one stretch of 16 sets ≤ 3 reads (exFAT) / 6 (FAT32); in 20-ms chore pieces (model) longest 14.7 ms, stretch ≤ 3.7 ms | `orpheus-index` `tests/index_piece.rs` `measure_index_piece` |
| 2026-11-25 | Firmware with `job:index` in stretches, worker worktree build | resident 80,108 B, `__image_end` 0x1138ec, `__bss_end` 0x131b00, `__stack_bottom` 0x131d00 (all unchanged); 108 code pages (was 107), slot 1,118,208 B; nested worst stack 21,812 of 22,016 B (was 21,820) | `xtask build`, `xtask stack --check`, `nm` |
| 2026-11-26 | Synth code paging, the three demo songs whole (unicorn harness of the per-piece work, mapped onto `layout.json`, 32-line cache model), "Etude" / "Hill Song" / "Sound tour" at 288 MHz | before (build `8240f128` layout): Etude 96 line loads a piece (80 in the render), `pg=` ≈ 240 % render only, 290 % in all; after (SYN-3 paging): 2.3 / 2.3 / 1.8 loads a piece, `pg=` render only 3.2 / 3.3 / 2.4 %, in all 6.9 / 7.0 / 5.3 % | model; on the player: lead's checks in the session entry |
| 2026-11-26 | Firmware with the UI font in two paged pieces, merged strings paged, cluster-start gaps filled (PG-1, resident room), worker worktree build | resident 81,876 → 56,036 B (`size` 56,033; rodata 37,167 → 11,325; text 28,304 and data 16,404 unchanged), `.bss` 124,296 B, code 125 → 124 pages (33,114 B of the window free; 7,458 B of cold code in cluster-start lines), rodata 28 → 30 of 48 pages, slot 1,339,392 → 1,323,008 B, stack usable 45,568 B, margin 5,212 → 31,068 B; opt-level `"z"` for 14 cold crates (measured, not kept): code −14.6 KB, rodata +5.4 KB | `xtask build`, `size --check`, `stack --check` |
| 2026-11-27 | Firmware update write path from a PG-2 image (`bdebff77`, SHA-256 cold on its constants' line) | 2.5 KB/s (32 KiB in 13.1 s); 5,300 line loads/s; 129 rodata faults per 64-byte block; 88.9 % of PC samples in `compress256` | `dev prof --pc --during "update … --stop-after 32768"` |
| 2026-11-27 | MIDI "Etude" on `bdebff77` | 2,297 line loads/s (1,170 rodata faults/s); underruns 2 (Hill Song 1, Sound tour 0) | `dev prof --faults 25`; `playdir …/midi`, `status` |
| 2026-11-27 | Idle Home on `bdebff77` | 199–219 line loads/s screen on (≈ 0.9 UI loops/s); 177/s screen off; 270/s with a MIDI file loaded paused | `dev stats` 5–10 s apart; `dev prof --faults 10` |
| 2026-11-27 | Clock after MIDI on `bdebff77` | Base 48 within 3 s of `stop` and of the folder's end | `dev stats` |
| 2026-11-27 | Firmware PG-2b, worker build | resident 60,336 B (+512: 505 B synth constants), code 127/128, rodata 46/64 (8 cold pages, 0 line conflicts), slot 1,482,752 B, stack margin 16,852 B | `xtask build`, `size`, `stack` |

## 5. Open questions for the user

* VER-7 (2026-11-25): the Authenticator says **"Vault damaged"**. The vault
  sector (NOR 0x7DE000) was never set up: it still holds the bytes that
  were there before the full-layout switch. Wiping it in the app
  (Authenticator, Menu › Wipe vault…, then a PIN) makes it a real vault;
  nothing is lost. The suite accepts every state and never writes it.
  Also: the player's clock is not set (Steps says "No clock: no history";
  `cargo xtask dev time --set` sets it).
* VER-12 (2026-11-24): the unplugged `mixed` soak cannot be driven over
  USB. Is a cable-in run with charging paused (`--battery`) an acceptable
  stand-in, or should the owner run an unplugged listening session with
  the drain logger (user check 19) as the battery part?

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
