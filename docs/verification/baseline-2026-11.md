# Verification baseline, 2026-11 (VER-1, VER-5 host side)

First measurement with the M11 tooling (07 §3.5–3.6, §4.4): where the
project's own host-side source stands against the 100 % requirement, what
the sanitizers say, which parsers have no fuzz target, and the work list for
VER-2 (unit tests), VER-3 (adversarial inputs) and VER-4 (fuzzing).

Measured on 2026-11-24 on the tree of commit `e92443f` plus the VER-1/VER-5
tooling (recorded as `e92443fd3003-dirty` in `metrics/coverage.csv`), with
`nightly-2026-11-05` (rustc 1.101, LLVM 23).

## 1. How the numbers were made

```
cargo xtask cover --fuzz --html                 # all host tests + fuzz corpus replays
cargo xtask cover --crate orpheus-img --add     # re-run after fixing a test race (§4)
cargo xtask cover --crate xtask --add --html    # xtask after its own changes
cargo xtask test asan --lean                    # ASan + LSan, one package at a time
cargo xtask test miri                           # Miri crate list (xtask/src/sanitize.rs)
cargo xtask metrics --elf <main checkout>/target/orpheus/firmware.elf
```

The root disk had 0.9–5 GB free during the session (other workers build in
parallel), which is why the full run was completed with `--add` re-runs
instead of a second full run, and ASan ran with `--lean`.

* Counted: `*.rs` under `crates/`, `sim/`, `xtask/`, without `tests/`
  directories and `tests.rs` modules (the instrument, not the subject).
  Regions and branches: union over every instantiation and binary.
* Not counted yet: `fw/` (device code, VER-6; list in §3), doctests, the
  codec conformance corpus run (`xtask test codecs`) and the RFC 8251 Opus
  vectors — neither `tests/corpus/` nor `tests/vectors/` exists on this
  machine, so the codec crates are measured by their unit tests only.
* Fuzz replays: there is **no committed corpus** (`tests/fuzz-corpus/` does
  not exist; `fuzz/corpus/` is git-ignored and absent). The only input
  replayed was `tests/regress/flac/nostop-36e2acf7`. Fuzz coverage is
  therefore effectively zero; VER-4 has to build and commit the corpora.

## 2. Coverage per crate (worst first)

Total: **regions 72.16 %** (60,678 / 84,088), **lines 71.78 %** (37,275 /
51,931), **branches 65.09 %** (7,470 / 11,476). Exclusions: none (the list
is empty). Files with function bodies but no coverage data: 2
(`sim/src/window.rs`, feature `window` not built by the tests;
`crates/orpheus-img/fuzz/fuzz_targets/thumb.rs`, no corpus).

| Crate | Regions | Lines | Branches | Main uncovered areas (uncovered / total regions) |
|---|---:|---:|---:|---|
| orpheus-host | 8.40 % | 7.13 % | 0.67 % | Everything that talks to a device: `adfu.rs` 322/322, `nor.rs` 227/227, `bot.rs` 183/183, `devchan.rs` 104/104, `sg.rs` 86/86, `device.rs` 80/80 |
| orpheus-codec-vorbis | 9.56 % | 8.67 % | 2.37 % | `setup.rs` 645/645, `audio.rs` 460/460, `lib.rs` 444 — the unit tests need the conformance corpus; without it only `math.rs` runs |
| orpheus-codec-conformance | 10.71 % | 12.80 % | 8.18 % | `checks.rs`, `main.rs`, `join.rs`, `compare.rs` never run (the binary is driven by `xtask test codecs`, not by tests) |
| xtask | 16.08 % | 17.28 % | 67.65 % | `test.rs` 2157/2157 (corpus generator, suites), `cover.rs`, `dev/*` (device), `pagelink.rs` 424/1089, `linkgraph.rs` 313/437, `selftest.rs` |
| orpheus-codecs | 29.17 % | 44.44 % | — | `open`/`inspect` dispatch per format |
| orpheus-codec-opus | 37.13 % | 34.44 % | 29.54 % | CELT (`bands.rs` 1208, `decoder.rs` 1018, `mdct.rs` 498, `rate.rs` 429), `opus_dec.rs` 670, `ogg_opus.rs` 322 — all never run without the RFC vectors |
| orpheus-codec-wav | 65.59 % | 67.58 % | 49.32 % | AIFF and AU headers (`header::aiff`, `au`, `aiff_comm` never run), QuickTime IMA ADPCM, sample conversions |
| orpheus-codec-mp3 | 74.08 % | 76.27 % | 55.52 % | Intensity/MS stereo (`stereo_process`, `intensity_stereo` never run), `header::find_frame`, mono synth paths |
| orpheus-codec-speex | 79.09 % | 78.17 % | 61.65 % | `skim_packet`/`nb::skim`, `nb::vocoder`, wideband decode branches |
| orpheus-ogg | 80.68 % | 82.04 % | 73.73 % | `ranges::Reader::next`, `peek_packets`, `seek::Bisect::step` error paths |
| orpheus-codec-flac | 80.90 % | 78.62 % | 64.68 % | Frame header edge cases, fixed predictors of order 0–4, `seek_step`, metadata blocks |
| orpheus-apps | 81.83 % | 79.87 % | 67.52 % | `screens/player.rs` 207/1318 (menu picks), `pictures.rs` 189/392, `lists.rs` 126/812 (`activate`), `library.rs` 113/331, `files.rs`, `services.rs` |
| orpheus-audio | 83.53 % | 81.85 % | 62.20 % | `probe.rs` 73/297 (container probe), CRC table builders (const-evaluated on device), `bits::utf8_u64` |
| orpheus-sim | 83.65 % | 84.29 % | 69.44 % | `media.rs` 172/1015 (`Media::command`), `apps.rs` 113/534, `demo.rs`, `main.rs` (never run), `library.rs` |
| orpheus-library | 89.26 % | 90.89 % | 77.44 % | `build.rs` 142/1995 (`Run::scan`, `drain`), `query.rs` 136/838 (`item`, `letter_runs`), `FileFormat::of` |
| orpheus-player | 89.46 % | 88.66 % | 65.89 % | `Player::command` (59 regions), `handle`, `image::decode_queue` rejects |
| orpheus-meta | 89.62 % | 89.78 % | 71.86 % | `riff::Riff::step` 39, `length::mpeg`/`aiff`, `id3` frame variants, `apply_key` |
| orpheus-fs | 89.94 % | 90.71 % | 81.11 % | `dir.rs` 199/1873, `volume.rs` 122/1253 (rename, undelete, grow_dir, upcase table), `file.rs` 99/827 (set_len) |
| orpheus-dsp | 91.17 % | 90.82 % | 84.49 % | EQ presets (`Preset::gains_db`), `Coeffs::from_f32`, soft-cascade ramps, meter |
| orpheus-ui | 91.75 % | 89.59 % | 78.73 % | `Font::parse` (never run), `Gfx::draw_char`/`stroke`, `icons::row`/`width`, `shell::apply_nav` |
| orpheus-input | 92.09 % | 88.06 % | 91.67 % | `Key::name`, `Key::from_index` (never run) |
| orpheus-index | 92.20 % | 93.54 % | 84.78 % | `letters::LetterTable::push`/`begin` (never run), `query::seek_letter`, `extsort` merge/suspend |
| orpheus-sdcard | 93.06 % | 89.17 % | 66.67 % | `Status::state`, `Csd::parse` v1 path, `Csd::bytes` |
| orpheus-pager | 93.45 % | 93.70 % | 80.43 % | `PagerCore::abort_load`, `pages`, `fault` error arms, `sim::Machine::check` |
| orpheus-proto | 93.60 % | 96.64 % | 80.56 % | `from_bytes` rejects (`fault.rs`, `mailbox.rs`, `result.rs`, `slot.rs`), `fault::Kind::name`, `logring::unroll` |
| orpheus-settings | 94.41 % | 95.91 % | 76.92 % | `Store::compact`/`scan`/`mount` error arms |
| orpheus-otp | 95.30 % | 94.73 % | 82.76 % | `vault.rs` 66/1137 (`change_secret`, `wipe` closures), `Entry` Debug, `Algorithm::name` |
| orpheus-usb | 96.16 % | 92.85 % | 78.57 % | `scsi::inquiry`/`mode_sense` variants, `bot.rs` phase errors, `devchan.rs` rejects, `ImageDevice` |
| orpheus-bt | 96.27 % | 95.03 % | 79.57 % | `Transmitter::cancel_connect`/`stop_inquiry`/`query_volume` (never run), `forget`, `Frame` Debug |
| orpheus-img | 96.50 % | 96.93 % | 87.77 % | `bmp::Bmp::header`/`rle_rows`, `jpeg::emit_rows`, `png::chunks`, `thumb::store` |
| orpheus-power | 97.97 % | 96.59 % | 86.36 % | `Charger::step` arms, `Default` |
| atj2157-pac | 98.74 % | 98.32 % | 66.67 % | `Reg::write`/`modify`/`set_bits`, `RoReg::read` (volatile access; host tests never touch a register), `adc::ch_digctl` |

Details: `target/orpheus/coverage/files.csv`, `functions.csv` (every
function with an uncovered region, worst first per crate), `html/`.

## 3. Device code not host-testable yet (`fw/`, VER-2 seams / VER-6)

None of the `fw/` workspace compiles on the host; all of it is 0 % until
VER-6 (device coverage) or until its logic moves behind HAL seams:

* `fw/firmware/src/`: `audio.rs` (1611 lines, playback path), `media.rs`
  (1665, card services, play queue), `services.rs` (935, the `orpheus-apps`
  service traits), `bt.rs` (729), `pictures.rs` (727), `library.rs` (607),
  `ui.rs` (434), `usbdev.rs` (431, developer channel), `main.rs` (279),
  `state.rs` (45). Most of `media.rs`, `services.rs`, `library.rs`,
  `pictures.rs` and the non-I/O parts of `audio.rs` and `bt.rs` are logic
  that VER-2 should move behind traits and test on the host.
* `fw/atj2157-hal/src/`: `usb.rs`, `sd.rs`, `nor.rs`, `rt.rs`, `uart.rs`,
  `i2c_soft.rs`, `dma.rs`, `persist.rs`, `time.rs`, `pager.rs`, `dac.rs`,
  `lcm.rs`, `clock.rs`, `wdt.rs`, `rtc_clock.rs`, `log.rs`, `interrupt.rs`,
  `executor.rs`, `lib.rs`, `gpio.rs`, `pwm.rs`, `cs.rs`, `pmu.rs`,
  `delay.rs` — hardware drivers (VER-6/VER-7); `persist.rs`, `time.rs`,
  `log.rs` and the pager glue contain host-testable arithmetic.
* `fw/yoton-bsp/src/`: `bt.rs`, `display.rs`, `tiny.rs`, `charger.rs`,
  `card.rs`, `settings.rs`, `pins.rs`, `keys.rs`, `lib.rs`.
* `fw/boot/stage2`, `fw/payloads/norprog`, `fw/bringup/*` (bring-up images).

## 4. Sanitizers

### AddressSanitizer + LeakSanitizer (`xtask test asan --lean`)

63 test binaries, 586 tests, all passed; no AddressSanitizer error (no
out-of-bounds, use-after-free, stack-use-after-return or init-order
report). All 14 fuzz targets ran their (nearly empty) corpora clean.
LeakSanitizer reported leaks at process exit in 4 binaries:

| Where | Report | Cause | State |
|---|---|---|---|
| `orpheus-apps` lib tests (`strings::tests::english_and_pack`) | 1,380 B in 2 allocations | the test `Box::leak`ed two language packs to get `&'static [u8]` for `Lang::from_pack` | **fixed**: the packs live in `OnceLock` statics (reachable); re-run clean |
| `orpheus-sim` `tests/apps.rs`, `tests/golden.rs`, `tests/media.rs` | 307,840 B / 3,469,440 B / 116,480 B, every allocation 8,320 B | `sim::media::Media::new` `Box::leak`s 16 `CacheSlot`s (16 × 520 B) per instance because `orpheus_fs::Volume` borrows its cache for `'static`; every `Media` dropped by a test loses it | **open** (VER-5): not local — needs `Volume` to hand its cache back (`into_parts`/`unmount` returning it) or a cache pool in the simulator. Simulator-only; the firmware's cache is a `static` |

Also found by the run (tooling, fixed in `xtask`): with the profile's
`panic = "abort"`, `-Zbuild-std` builds a std the test harness cannot link
(fixed with `panic = "unwind"` for the ASan build); `mount_walk` did not
find its seed images under `<target>/<triple>/tmp/` (fixed in
`cover::fs_seeds`).

### Miri (`xtask test miri`)

**No undefined behaviour** in 14 crates, 232 tests (isolation on; 16 tests
skipped by name, listed with reasons in `MIRI` in `xtask/src/sanitize.rs`;
ASan runs all of them):

| Crate | Tests under Miri | Skipped |
|---|---:|---|
| atj2157-pac (`unsafe`: volatile `Reg`) | 70 | — |
| orpheus-host (`unsafe`: SG_IO ioctl, not reached by the unit tests) | 1 | `png_roundtrip` (writes a file) |
| orpheus-pager | 3 | — (its trace tests are integration tests) |
| orpheus-proto | 22 | — |
| orpheus-otp | 15 | `kdf::…stepping_equals_one_shot`, `uri::…fuzz_smoke`, 9 × `vault::tests` (KDF per unlock: > 15 min) |
| orpheus-settings | 3 | `power_cut_at_every_step` (> 5 min) |
| orpheus-input / -power / -sdcard | 6 / 9 / 5 | — |
| orpheus-usb | 31 | `iso_reads_across_runs_with_padding` (> 10 min) |
| orpheus-bt | 30 (≈ 8 min) | — |
| orpheus-ogg | 10 (≈ 8 min) | — |
| orpheus-meta | 18 | `corpus_files` (reads files), `garbage_never_panics_or_spins` (> 10 min) |
| orpheus-audio | 9 | — |

Not under Miri yet: the integration tests of every crate (file system
images, generated pictures), the codec crates, `orpheus-fs`, `orpheus-img`,
`orpheus-index`, `orpheus-library`, `orpheus-player`, `orpheus-dsp`,
`orpheus-ui`, `orpheus-apps`, `orpheus-sim`, `xtask` — all
`#![forbid(unsafe_code)]` except `xtask`/`sim` (no `unsafe` either); VER-5
follow-up: a reduced-size test configuration (`cfg(miri)` iteration counts)
so the slow tests and these crates fit a time box.

### Fixed in this session

* `crates/orpheus-img/tests/common/mod.rs`: the conformance tests of one
  binary ran `tests/gen.py` concurrently into the same `gen-<pid>` scratch
  directory when the generated pictures were missing (fresh target
  directory): five tests failed with truncated images. Generation is now
  serialised by a mutex (test-only change).

## 5. Fuzz-target inventory

Existing cargo-fuzz targets (14; nightly, `xtask test fuzz-smoke` runs each
60 s):

| Fuzz project | Targets | What they reach |
|---|---|---|
| `crates/codecs/conformance/fuzz` | `decode_any`, `wav`, `mp3`, `flac`, `vorbis`, `ogg`, `opus`, `speex`, `meta`, `lyrics`, `probe` | codec frame entries, Ogg pages, container probe, tag parsers (ID3, Vorbis comment, MP4, RIFF), lyrics |
| `crates/orpheus-fs/fuzz` | `mount_walk` | FAT12/16/32 and exFAT mount, directory walk, some writes on mutated images |
| `crates/orpheus-img/fuzz` | `decode`, `thumb` | JPEG/PNG/GIF/BMP/QOI decoders, thumbnail cache files |

Corpora: none committed. Regression inputs: `tests/regress/flac/` (1).
Stable mutation smoke runs exist in `orpheus-codec-conformance fuzz-smoke`,
`orpheus-fs/tests/fuzz_smoke.rs` and `orpheus-otp` `uri::tests::fuzz_smoke`.

Parsers and state machines that take outside data and have **no fuzz
target yet**:

| Crate | Entry points |
|---|---|
| orpheus-otp | `uri::parse` (otpauth URIs from the card), `base32::decode`, vault records on NOR (`Vault` mount/unlock) |
| orpheus-bt | `frame::Decoder::feed` (UART byte stream from the module), `proto` event parsing (`Event::parse`), `Transmitter` state machine |
| orpheus-usb | `msc::bot::Cbw::parse`, SCSI/MMC command handling (`msc::scsi`, `msc::mmc`), developer-channel requests (`devchan`) |
| orpheus-proto | `from_bytes` of `devchan`, `fault::FaultRecord`, `mailbox::Mailbox`, `result::ResultBlock`, `slot::SlotHeader`; `logring::unroll` (NOR/RAM contents after a crash) |
| orpheus-settings | `Store::mount` over arbitrary settings-sector contents |
| orpheus-index | index files on the card (`record::Header::decode`, `query::Index::open`, letter runs) |
| orpheus-library | library files `lib-a/b.ydb` (`format::Header::decode`, `CatFixed::decode`, `query::Library::open`) |
| orpheus-player | `/.orpheus/queue.bin` (`image::decode_queue`), `player.bin` records (`store::*::decode`) |
| orpheus-sdcard | `Cid::parse`, `Csd::parse`, `Scr` (card registers) |
| orpheus-apps | `settings::decode`, `browse::FileRef::from_bytes`; UI key sequences over every screen (07 §3.8: a simulator fuzz target) |
| orpheus-input, orpheus-power | key-ladder decoding and charger/gauge state machines over arbitrary sample sequences |
| orpheus-audio | `probe` is reached through `probe`; `stream`/`source` ring behaviour under arbitrary read sizes has no target |
| orpheus-pager | random access traces against the simulated MPU (07 §3.1; a property test or fuzz target) |
| orpheus-host, xtask | device responses (ADFU/BOT/ymon), `layout.json`, ELF inputs — host tools, lower priority |

## 6. Work list per crate

VER-2 = unit tests to 100 %, VER-3 = adversarial-input tables and
`proptest` properties, VER-4 = fuzz targets and corpora.

**Tooling first (VER-1/VER-4, before any per-crate work is measured):**
commit seed corpora under `tests/fuzz-corpus/<target>/` for all 14 targets;
let `xtask cover` drive the conformance binary over `tests/corpus/` and the
Opus vectors when present; build `orpheus-sim` with `--features window`
in the coverage run (or move the window code behind a testable seam).

| Crate | VER-2 | VER-3 | VER-4 |
|---|---|---|---|
| orpheus-host | Mock `Transport` (SG_IO, USB) so `adfu`, `bot`, `nor`, `devchan`, `device` run against recorded exchanges | Truncated/garbled device responses, wrong CSW tags/status | Optional: fuzz response parsers |
| orpheus-codec-vorbis/-opus | Run with the conformance corpus and RFC 8251 vectors inside `xtask cover`; unit tests for setup header variants (floor 0, residue types), CELT/SILK mode switches, PLC | Truncated setup/identification headers, oversized codebooks, arena one byte too small | Seed `vorbis`/`opus` corpora from the conformance files |
| orpheus-codec-conformance | Exercised by `xtask cover` running the binary (tooling) | — | — |
| xtask | Unit tests for the pure parts: `pagelink` packing, `linkgraph`, `size` budgets, `cover` analysis (`analyse` on a canned export), `metrics::parse_frames`, `test.rs` manifest/seed helpers; `dev/*` against a mock device | Malformed `layout.json`, maps, ELF | — |
| orpheus-codecs | Dispatch tests per format, `unsupported`, `arena_used` | Mis-labelled files (extension vs content) | `decode_any` covers it once seeded |
| orpheus-codec-wav | AIFF/AIFF-C/AU headers, QuickTime IMA, every sample format conversion | Every chunk length 0/max/max+1, odd padding, missing `fmt`/`COMM` | Seed `wav` with AIFF/AU |
| orpheus-codec-mp3 | MS and intensity stereo streams, mono synth, `find_frame` resync | Free-format, bad CRC, reservoir underflow, Xing/LAME tag lies | Seed `mp3` |
| orpheus-codec-speex | Wideband/UWB, vocoder mode, `skim` | Truncated frames, invalid modes | Seed `speex` |
| orpheus-ogg | Page reader error paths, bisect seek with gaps | Page size 0/255×255, CRC errors, continued packets across many pages, stream switches | `ogg` corpus |
| orpheus-codec-flac | Fixed predictors 0–4, all header block sizes/rates, seek tables, metadata blocks | Every header field invalid, CRC-8/16 errors, truncated subframes | Keep `tests/regress/flac/`, seed `flac` |
| orpheus-apps | Drive every menu pick and list action of Now Playing, Pictures, Lists, Library, Files through `nav::Screen` with mock services | Services returning errors/empty/oversized lists | UI key-sequence fuzz target in the simulator (07 §3.8) |
| orpheus-audio | `probe` per container, CRC table builders at run time, `utf8_u64` all lengths | Probe on every truncation of each container header | `probe` target seeded |
| orpheus-sim | `Media::command` per request, `HostServices::set`, `main`/`run` via a scripted run | Hostile script files | Seeds for the UI fuzz target |
| orpheus-library | `Run::scan` with unreadable/odd files, `drain`, `item`, `letter_runs`, `FileFormat::of` per extension | Corrupted `.ydb` headers and category tables, interrupted builds at every step | New target: library file open/query |
| orpheus-player | Every `Command` variant in every state | `queue.bin`/`player.bin` truncated, wrong CRC, out-of-range indices | New target: queue/record decode |
| orpheus-meta | RIFF `INFO`/`id3 ` chunks, MPEG length without Xing, AIFF length, ID3v2.2/2.3/2.4 frame variants | Deep/oversized frames, unsync, bad text encodings | `meta` seeded with tagged files |
| orpheus-fs | rename/undelete/grow_dir, upcase table, `set_len` on FAT and exFAT, host helpers | Power cut at every write (exists for some paths), hostile names, cyclic FAT chains | `mount_walk` corpus from the seed images |
| orpheus-dsp | EQ presets, `Coeffs::from_f32`, soft cascade ramps, meter | Parameter extremes, NaN/inf in float paths | — (proptest) |
| orpheus-ui | `Font::parse`, `Gfx::stroke`, icon helpers, `Shell::apply_nav` | Malformed font data, zero-size widgets | UI sequences (simulator) |
| orpheus-input | `Key::name`, `from_index` | ADC noise sequences (proptest) | Sample-sequence target |
| orpheus-index | `LetterTable`, `seek_letter`, extsort suspend/merge | Corrupted index files, suspend at every step | New target: index open/query |
| orpheus-sdcard | `Status::state`, CSD v1, `Csd::bytes` | All register bit patterns (exhaustive over fields) | Small target or proptest |
| orpheus-pager | `abort_load`, `pages`, fault error arms | Random traces (proptest) | — |
| orpheus-proto | Every `from_bytes` reject, `Kind::name`, `unroll` wrap cases | Every truncation/magic/CRC error per record | New target: all `from_bytes` |
| orpheus-settings | compact/scan/mount error arms | Arbitrary sector contents | New target: mount on random sectors |
| orpheus-otp | `change_secret`, `wipe`, Debug impls, `Algorithm::name` | URI parameter extremes (partly done) | New targets: `uri::parse`, vault mount |
| orpheus-usb | INQUIRY/MODE SENSE variants, BOT phase errors, devchan rejects, `ImageDevice` | Every CDB with bad lengths/LBAs, commands in every order | New target: SCSI/BOT command stream |
| orpheus-bt | `cancel_connect`, `stop_inquiry`, `query_volume`, `forget`, Debug | Frames split at every byte, bad checksums, events in every order | New target: frame decoder + transmitter |
| orpheus-img | BMP header variants and RLE, JPEG row emission edge cases, PNG chunk errors, thumb store | Already broad; add arena-one-byte-short per format | Seed `decode`/`thumb` from `target/tmp/orpheus-img/data-*` |
| orpheus-power | Remaining `Charger::step` arms, `Default` | Sensor sequences (proptest) | — |
| atj2157-pac | `Reg`/`RoReg` accessors through a host-memory register (pointer to a test `u32`) | — | — |

## 7. Metrics recorded

`metrics/coverage.csv`: 34 rows (33 crates + `TOTAL`) for this baseline.
`metrics/static.csv` (row commit `deac5f96df1e`): resident 79,087 B, bss
121,440 B, stack section 25,768 B, 7 panic sites, 73 code + 16 rodata
pages, full-layout slot 815,104 B, and the 40 largest prologue stack frames
(largest: `firmware::audio::init_chain` 11,192 B, the audio task 5,080 B,
the UI task poll 4,776 B, `firmware::ui::new_ui` 4,504 B) — from the main
checkout's last `xtask build` at the time (layout build id `3ed98285…`),
not a build of this worktree. The coverage rows carry the commit the
coverage run measured (`e92443fd3003-dirty`: `e92443f` plus the
uncommitted VER-1/VER-5 tooling).
