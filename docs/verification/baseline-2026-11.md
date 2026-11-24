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
| `orpheus-sim` `tests/apps.rs`, `tests/golden.rs`, `tests/media.rs` | 307,840 B / 3,469,440 B / 116,480 B, every allocation 8,320 B | `sim::media::Media::new` `Box::leak`s 16 `CacheSlot`s (16 × 520 B) per instance because `orpheus_fs::Volume` borrows its cache for `'static`; every `Media` dropped by a test loses it | **fixed in VER-4** (§8.4, S-1): `orpheus_fs::Volume::into_parts` hands the slots back and dropped `Media` return them to a pool the next one reuses. Simulator-only; the firmware's cache is a `static` |

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


## 8. VER-4 part 1: fuzz targets, committed corpora, findings (2026-11-24)

Measured on commit `0a2598d` plus the `--fuzz-only` flag fix and this
document (recorded as `0a2598dd44a2-dirty`), nightly-2026-11-05,
cargo-fuzz 0.13.1, AddressSanitizer (cargo-fuzz's default) in every fuzz
run.

### 8.1 Targets

30 cargo-fuzz targets (14 before, 16 new). The new ones, with what they
check besides "no panic, no hang":

| Target (project) | Input | Invariants |
|---|---|---|
| `otp_uri` (`orpheus-otp/fuzz`) | `otpauth://` URI / base32 text | parsed entry within the entry limits, codes of `digits` digits; the canonical URI written from it parses to the same entry; base32 encode/decode round trip both ways |
| `otp_vault` | vault sector + operations (push, remove, set counter, HOTP step, save, change PIN, remount) | `load` classifies erased/damaged/locked by the header; aligned programs; a wrong key always fails and arms the back-off; after save + remount + unlock exactly the model's entries; the HOTP code is the pre-step counter's |
| `bt_frame` (`orpheus-bt/fuzz`) | UART bytes, chunk size | frames independent of chunking; statistics count them; every frame re-encodes to bytes decoding to itself, a run of them to the same run; event parsing agrees with the frame; volume scales in range and round-trip |
| `bt_tx` | module bytes, well-formed events, acknowledges, time, every firmware request | every frame sent well formed; no restricted command (rule 6) except 0x24 after an accepted `shutdown`; nothing sent while off; list and name bounds; output indices exist; outputs drain; a "no absolute volume" report leaves the volume |
| `usb_scsi` (`orpheus-usb/fuzz`) | CBWs, data, raw packets for disk / read-only / absent / CD / DVD units | medium accesses inside the medium, none on read-only or absent units (medium unchanged then); CSW tag, status, residue; data ≤ transfer length; plans inside the medium, failures with sense data, standard allocation lengths, padded head ≤ length |
| `usb_devchan` | setup packets + data stages | only vendor/device requests claimed; replies ≤ `wLength` and the buffer; PEEK only in development builds; reboot only for REBOOT; key pairs as sent (≤ 16); INFO round trip |
| `proto_records` (`orpheus-proto/fuzz`) | NOR/RAM contents at an offset | each record that parses re-encodes to one that parses the same (mailbox, INFO: to the input bytes; fault record and slot header: every kept field); the log ring unrolls to a rotation within capacity and output; word decoders round-trip |
| `settings_mount` (`orpheus-settings/fuzz`) | four settings sectors + sets, removes, remounts, power cuts | mount always succeeds; listed keys read back once; never programs over non-erased bits, only aligned words; reads = model; a remount keeps the model; a cut leaves the old or the new value of that key only |
| `index_open` (`orpheus-index/fuzz`) | index file (header CRC and length optionally repaired) | opens only for its directory, spec and length; row windows = single rows = positions; letter jumps inside the listing; the RAM letter table answers like the file; a verified file lists directories first |
| `library_open` (`orpheus-library/fuzz`) | library file in slot A or B (header CRC and length optionally repaired, second slot) | row windows of the asked size; names are text; letter jumps inside the lists; queues ≤ asked; Shuffle all ≤ tracks; on a verified file positions and lookups by locator agree |
| `player_queue` (`orpheus-player/fuzz`) | `queue.bin`, capacity | decodes only what fits, identity matches, re-encodes to the same bytes; locators round-trip; no cut-short file decodes |
| `player_resume` | `player.bin` + commands and decoder/reader events | records re-encode equal; every Open/Prefetch item exists; position inside the queue; the shuffle order is a permutation of the queue; ≤ 16 bookmarks; a written `player.bin` restarts to the same record and bookmarks |
| `sd_registers` (`orpheus-sdcard/fuzz`) | CID/CSD registers, R1/OCR words | bit fields = the 128-bit value's bits; CID fields = the bytes; a CSD parses iff its structure is 1–3, capacity per the version's formula, byte size without overflow; status/OCR flags are their bits |
| `input_samples` (`orpheus-input/fuzz`) | ADC ladder samples and the power line | ≤ 1 key per ladder; the earphone band is never a key; changes only after two equal samples, each edge flips its key, edges in key order, held set = applied edges |
| `power_samples` (`orpheus-power/fuzz`) | charger status, battery ADC, cable, presence results | charge code never above the 200 mA code (rule 3) and only stock codes; no cable ⇒ stopped, no current; detection only at cable insertion; no current in Full/Fault/NoBattery; percentage monotonic in 0–100; smoothed reading within the raw range |
| `ui_keys` (`sim/fuzz`) | taps, holds, presses/releases, waits, system events (`set:`), settings by id with any value, appearance, language, jumps to screens | navigation depth 1–8; dirty rectangles inside the panel; a long Back (no dialog, unlocked, screen on) ends at Home; a full redraw equals the incremental frame (golden-consistent redraws; skipped for the rate-limited Levels view and the open F-UI-1 corners) |

The 14 old targets kept their bodies (the `orpheus-img` ones moved into a
shared `src/lib.rs`).

### 8.2 Corpora and runs

Committed corpora: 30 targets, 13.5 k files, 15 MB, every file ≤ 64 KiB
(`tests/fuzz-corpus/<target>/`); dictionaries for `otp_uri`, `otp_vault`,
`bt_frame`, `usb_scsi`, `proto_records`, `settings_mount`, `index_open`,
`library_open`. Seeds from the crates' encoders and fixtures and from the
synthetic conformance corpus (`xtask test fuzz-seeds`). Runs: every new
target 10 min after its last fix (`ui_keys`: five 10-minute runs, each
ended by a finding, see §8.4), every old target 5 min; corpora minimised
with `cargo fuzz cmin`. The codec and picture corpora grew to 2–8 MB each
and were re-merged with `-max_len=4096` to stay under 20 MB (FLAC lost
most of its grown corpus that way; the seeds were written back).

Executions in the last run of each target (ASan): `otp_uri` 22.2 M
(37 k/s), `otp_vault` 179 k (298/s, one KDF per unlock), `bt_frame` 1.6 M,
`bt_tx` 3.7 M, `usb_scsi` 0.9 M, `usb_devchan` 20.7 M, `proto_records`
55.7 M, `settings_mount` 161 k, `index_open` 1.1 M, `library_open` 0.6 M,
`player_queue` 4.0 M, `player_resume` 2.3 M, `sd_registers` 2.2 M,
`input_samples` 19.3 M, `power_samples` 12.1 M, `ui_keys` 1–10/s (a
simulator per input; the simulator's library build costs ≈ 0.35 s); in
5 min the codec targets 10 k (FLAC, Opus) to 5.9 M (`meta`), `mount_walk`
75 k, `decode` 41 k, `thumb` 0.7 M. These are bounded runs, not the
plateau runs of 08 (≥ 24 h per target): part 2.

### 8.3 Coverage per crate, before and after

"Fuzz only": `xtask cover --fuzz-only` (new), the corpus replays alone;
percentages are of the files some fuzz binary contains (66 project files
are in none; total 48,171 / 61,729 regions). Before VER-4 there was no
corpus: the only fuzz input was `tests/regress/flac/nostop-36e2acf7`, so
fuzz coverage was ≈ 0. "Host + fuzz": `xtask cover` (the host tests,
which now also replay every corpus on stable, and the fuzz replays); the
"before" column is §2, measured on the tree of `e92443f` (later packages
added code, e.g. the simulator's Bluetooth fakes, which is why
`orpheus-sim`'s percentage fell).

| Crate | Fuzz only, before | Fuzz only, after: regions / lines / branches (covered/total regions of the files a fuzz binary contains) | Host + fuzz, before (regions) | Host + fuzz, after (regions / lines / branches) |
|---|---:|---:|---:|---:|
| atj2157-pac | 0 | — (in no fuzz binary) | 98.74 % | 98.74 / 98.32 / 66.67 % |
| orpheus-apps | 0 | 66.47 / 65.99 / 55.70 % (3993/6007) | 81.83 % | 86.01 / 83.75 / 71.60 % |
| orpheus-audio | ≈ 0 (one input) | 67.64 / 68.39 / 76.97 % (581/859) | 83.53 % | 92.92 / 92.84 / 85.92 % |
| orpheus-bt | 0 | 80.76 / 81.41 / 84.93 % (1146/1419) | 96.27 % | 98.34 / 97.91 / 87.50 % |
| orpheus-codec-conformance | ≈ 0 (one input) | 23.08 / 24.86 / 15.77 % (295/1278) | 10.71 % | 30.01 / 31.57 / 24.65 % |
| orpheus-codec-flac | ≈ 0 (one input) | 83.82 / 85.69 / 74.77 % (689/822) | 80.90 % | 86.86 / 88.34 / 76.61 % |
| orpheus-codec-mp3 | 0 | 89.51 / 89.00 / 80.39 % (2048/2288) | 74.08 % | 89.82 / 89.22 / 80.94 % |
| orpheus-codec-opus | 0 | 95.82 / 95.83 / 88.26 % (8060/8412) | 37.13 % | 95.56 / 95.56 / 89.11 % |
| orpheus-codec-speex | 0 | 87.40 / 86.23 / 76.29 % (1734/1984) | 79.09 % | 88.84 / 87.61 / 78.69 % |
| orpheus-codec-vorbis | 0 | 90.41 / 94.30 / 80.18 % (1583/1751) | 9.56 % | 90.66 / 94.42 / 80.18 % |
| orpheus-codec-wav | 0 | 86.96 / 91.31 / 80.48 % (1040/1196) | 65.59 % | 89.35 / 92.77 / 82.31 % |
| orpheus-codecs | ≈ 0 (one input) | 88.89 / 88.89 / 100.00 % (64/72) | 29.17 % | 94.44 / 95.56 / 100.00 % |
| orpheus-dsp | 0 | — (in no fuzz binary) | 91.17 % | 91.17 / 90.82 / 84.49 % |
| orpheus-fs | 0 | 80.67 / 82.06 / 76.47 % (3686/4569) | 89.94 % | 91.68 / 92.32 / 84.53 % |
| orpheus-host | 0 | 0.00 / 0.00 / 0.00 % (0/1088) | 8.40 % | 8.40 / 7.13 / 0.67 % |
| orpheus-img | 0 | 84.14 / 85.10 / 73.13 % (4297/5107) | 96.50 % | 96.94 / 97.41 / 90.40 % |
| orpheus-index | 0 | 58.57 / 61.17 / 71.83 % (1626/2776) | 92.20 % | 93.45 / 95.32 / 87.43 % |
| orpheus-input | 0 | 96.51 / 95.95 / 100.00 % (83/86) | 92.09 % | 100.00 / 100.00 / 100.00 % |
| orpheus-library | 0 | 83.43 / 85.19 / 67.14 % (2935/3518) | 89.26 % | 91.80 / 93.69 / 81.00 % |
| orpheus-meta | 0 | 70.45 / 74.56 / 64.01 % (1416/2010) | 89.62 % | 93.15 / 92.42 / 81.75 % |
| orpheus-ogg | 0 | 83.55 / 85.45 / 77.97 % (640/766) | 80.68 % | 89.69 / 89.16 / 84.32 % |
| orpheus-otp | 0 | 85.53 / 87.53 / 81.82 % (975/1140) | 95.30 % | 96.72 / 96.82 / 89.66 % |
| orpheus-pager | 0 | — (in no fuzz binary) | 93.45 % | 93.45 / 93.70 / 80.43 % |
| orpheus-player | 0 | 80.64 / 80.00 / 89.50 % (1029/1276) | 89.46 % | 97.00 / 96.68 / 87.30 % |
| orpheus-power | 0 | 97.69 / 97.30 / 100.00 % (127/130) | 97.97 % | 99.13 / 98.54 / 95.45 % |
| orpheus-proto | 0 | 86.14 / 84.01 / 66.67 % (522/606) | 93.60 % | 97.55 / 99.81 / 94.44 % |
| orpheus-sdcard | 0 | 100.00 / 100.00 / 100.00 % (116/116) | 93.06 % | 100.00 / 100.00 / 66.67 % |
| orpheus-settings | 0 | 94.30 / 96.76 / 87.50 % (364/386) | 94.41 % | 95.86 / 97.84 / 88.10 % |
| orpheus-sim | 0 | 49.21 / 53.52 / 48.92 % (1937/3936) | 83.65 % | 75.61 / 79.30 / 67.57 % |
| orpheus-ui | 0 | 83.05 / 82.58 / 73.77 % (3233/3893) | 91.75 % | 93.22 / 91.85 / 82.12 % |
| orpheus-usb | 0 | 83.83 / 81.13 / 88.02 % (1042/1243) | 96.16 % | 98.19 / 96.17 / 94.20 % |
| xtask | 0 | — (in no fuzz binary) | 16.08 % | 16.75 / 18.01 / 70.00 % |
| TOTAL | ≈ 0 | 78.04 / 78.44 / 72.37 % (48171/61729) | 72.16 % | 83.27 / 83.37 / 80.81 % |

Rows in `metrics/coverage.csv` (sources `fuzz` and `host+fuzz`).

### 8.4 Findings

Every reproducer is kept: fixed ones under `tests/regress/<target>/`,
replayed on every `cargo test` by the crate's `fuzz_corpus` test (and by
`xtask cover --fuzz` and `xtask test asan`); open ones that still fail
under `tests/regress-open/<target>/` (moved to `tests/regress/` with the
fix).

| Id | Where | What | Reproducer | State |
|---|---|---|---|---|
| F-BT-1 | `orpheus-bt` `proto::volume_from_sink`, `Transmitter` (event 0x26) | A sink volume above AVRCP's 127 (the module's "no absolute volume", 128 from a BlueZ sink) mapped to a player volume of up to 62 on the 0–31 scale and was stored as the player's volume: 128 became 31, the maximum, pushed to the next sink at its connection | `tests/regress/bt_tx/sink-volume-above-127`, `tests/regress/bt_frame/volume-from-sink-above-127` | **fixed**: clamped to 127; a report above 127 is passed on but leaves the volume |
| F-SET-1 | `orpheus-settings` `Store::scan` | Bytes programmed after the end-of-log marker (foreign data, bit rot) were programmed over by the next record (NOR only clears bits): the value read back corrupted and was lost at the next mount | `tests/regress/settings_mount/program-over-garbage-after-log-end`; unit test `garbage_after_the_log_end` | **fixed**: a non-erased tail counts as full, the next write compacts into an erased sector |
| F-SET-2 | `orpheus-settings` `Store::remove` | On a full store a key cannot be removed: the deletion marker needs room and compaction counts the removed value as live, so `remove` returns `Error::Full` and the value stays | `tests/regress/settings_mount/remove-when-full` (the target accepts `Full`) | **open** (an error, not corruption). Idea: drop the key from the index before compacting for a deletion |
| F-USB-1 | `orpheus-usb` `msc::mmc` READ DVD STRUCTURE | With an allocation length below 21 the plan's head (21 bytes) was longer than its length, against `Plan::DataInPadded` (the transport clipped it) | `tests/regress/usb_scsi/dvd-structure-head-over-allocation` | **fixed** |
| F-IDX-1 | `orpheus-index` `Index::seek_letter`, `LetterTable::load`/`push`; `orpheus-library` `Library::seek_letter` | A letter jump returned a run's first row as stored, unchecked: a damaged index or library moved the UI past the end of the listing; the RAM letter table accepted unordered runs | `tests/regress/index_open/letter-jump-outside-listing` | **fixed**: `Error::Corrupt`; the table refuses runs not in row order inside the listing |
| F-LIB-1 | `orpheus-library` queries | `u32` row offsets computed from file values (`id * TRK_ROW`, `(first + i) * 4`, `l.first + first + k`) overflowed on a damaged file (a panic with overflow checks, a wrapped read without); `text` underflowed when the pool end in the header lies past the pool section | `tests/regress/library_open/row-offset-overflow` | **fixed**: 64-bit offsets checked against the section, row ids checked |
| F-LIB-2 | `orpheus-library` `Library::file_len` | Below the header size when every section ends inside the header | `tests/regress/library_open/file-len-below-header` | **fixed** |
| F-PLY-1 | `orpheus-player` `PlayQueue { len: 0 }` | The previous shuffle order stayed: `order()` described N items of an empty queue | `tests/regress/player_resume/empty-queue-keeps-old-shuffle` | **fixed** |
| F-PLY-2 | `orpheus-player` `next_pos` (repeat one) | A bookmark played with an empty queue and repeat one: at its end the player asked to open (and read ahead) queue item 0 of the empty queue | `tests/regress/player_resume/repeat-one-bookmark-empty-queue` | **fixed**: repeat one only for a position the queue has |
| F-UI-1 | `orpheus-ui` `VolumeOverlay::draw` | The overlay's anti-aliased corners blend over what is under them and compound when the overlay is redrawn over itself (each volume step); the committed `volume_*` goldens show compounded corners. Cosmetic | `tests/regress/ui_keys/volume-overlay-corners` | **open**: needs a UI decision (corners over the background, as the toast does, or the content repainted first) and re-blessed goldens; the `ui_keys` oracle skips the four corner squares while an overlay is up |
| F-UI-2 | `orpheus-ui` `Shell::draw` | Content, a menu or a dialog redrawn under a shown volume overlay or toast covered it until it expired (e.g. a focus moving under the overlay) | `tests/regress/ui_keys/content-redrawn-over-volume-overlay`, `menu-redrawn-over-volume-overlay` | **fixed**: the overlays are drawn again when anything drawn in the frame touched them (`Gfx::drew_into`) |
| F-UI-3 | `orpheus-apps` Now Playing | The remaining time came from the position in ms while the row redraws once a second: it showed a second off until the next redraw | `tests/regress/ui_keys/remaining-time-off-by-one` | **fixed**: from the whole elapsed seconds |
| F-UI-4 | `orpheus-apps` Now Playing (incremental redraw) | Last `ui_keys` run: Music › OK (the YOASOBI queue, a two-line Japanese title), 2.85 s later a full redraw differs from the incremental frame in 16 pixels at x 6–9, y 94–97, just above the elapsed time (the once-a-second progress-row redraw and the full layout disagree there); not analysed further | `tests/regress-open/ui_keys/now-playing-redraw-left-of-elapsed-time` (`goto:Music ok wait:2850`) | **open** |
| S-1 | `orpheus-sim` `Media::new` | VER-5 LeakSanitizer: every `Media` leaked its cache slots (§4) | — | **fixed**: `Volume::into_parts`, a slot pool in the simulator |

Oracle corrections (the target was wrong, not the code; no reproducer
kept): a REQUEST SENSE CDB too short to hold the allocation length gets 18
bytes by design; the RAM letter table reads a letter that is not a
character as "no letter" while the file query stops there (compared on
verified files only); a header repaired by the target may carry any
counts (the verified-file oracles apply to unrepaired headers only);
`Command::Requeue` keeps the current position inside a non-empty queue by
contract (`queue::edit`); Now Playing's Levels view redraws at most every
66 ms by design.
