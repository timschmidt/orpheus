# 07 — Tooling, testing and verification

The worker building this firmware cannot see the screen, hear the headphones or
press a key. Everything in this document exists so that progress can still be
**proved**: on the desktop wherever possible, and on the real player through
USB with no human in the loop. Build this tooling first (milestone M0) and
extend it with every driver; a feature without an automated check is not done.

## 1. What already exists (reuse, then replace)

In `/home/tim/Documents/GitHub/yoton/` (the research repository; treat it as
read-only reference, do not commit to it):

| Item | What it does | Fate in Orpheus |
|---|---|---|
| `yoton.py`, `actions_probe.py` | ADFU (boot-ROM recovery) transport over USB, `ymon` RAM monitor upload, read/write any address, run payloads, NOR dump/program, reboot helpers, RGB565 → PNG | Port to Rust as the `orpheus-host` library + `cargo xtask dev …` (WP-T-2). Until the port passes its tests, call the Python through `/home/tim/Documents/GitHub/yoton/.venv/bin/python` |
| `payloads/ymon.c` (+ `ymon_s.S`) | The monitor that runs in RAM at `0x118000` under the boot ROM: memory access, fault-protected calls, host abort | Use the built `ymon.bin` at first; rewrite as a `no_std` Rust payload in WP-T-6 so that no C remains in the tree |
| `payloads/norprog.c`, `norprog0.c`, `nordump.c`, `spinor.h` | NOR erase/program/read payloads; `norprog` refuses addresses outside `0x762000–0x7DFFFF`, `norprog0` handles sector 0 | Same: reuse, then port. **Keep the address guards** |
| `payloads/ym03boot1.c`, `ym03stub.c` | The two-stage NOR boot stub now installed on the player (it boots Rockbox) | Replaced by Rust stages in 02-architecture §"Boot chain" |
| `tools/ym03_pagelink.py` | Two-pass link that splits Rockbox into a resident image and 8 KiB pages | Model for `xtask pagelink` (02 §"Demand paging") |
| `tools/ym03_maintest.py` | Boots a build from RAM in "test mode", lets it run, reads results and the frame buffer back after the self-reboot | Model for the HIL harness (§5) |
| `tools/ym03_prof.py`, `ym03_cpuprof.py`, `ym03_hot.txt` | Fault-log and PC-sample profiling, hot-section list for page packing | Model for `xtask dev prof` / `hot.toml` |
| `dumps/nor_backup_8m.bin` (sha256 `ad45687e…`), `dumps/nor_descrambled_8m.bin` | Full backups of the vendor NOR image | **Never delete.** `xtask dev backup-verify` checks they exist before any NOR write |
| `99-actions-yoton.rules` | udev rule giving the user access to `10d6:10d6` | Already installed on this host |

Host facts (checked 2026-11-23): Fedora 43; `rustc 1.98.1` stable, `cargo`,
`probe-rs` (no probe attached; SWD pads unknown), `arm-none-eabi-gcc/objdump`,
`ffmpeg`, `sox`, `openocd`, Python 3.14 with pyusb in the yoton venv. The
`thumbv7em-none-eabihf` target is **not** installed yet
(`rustup target add thumbv7em-none-eabihf`).

Device state at the time of writing: attached over USB, running the Rockbox
port in mass-storage mode (`10d6:1157`, 256 GB card with the owner's music —
**treat the card contents as precious**, see §7).

## 2. Repository tooling (`cargo xtask`)

One entry point; every sub-command is non-interactive, returns a non-zero exit
code on failure and prints a one-line verdict last. Sub-commands marked *(dev)*
talk to the player.

| Command | Purpose |
|---|---|
| `xtask build [--release] [--layout dual\|full] [--features …]` | Build the firmware ELF, then run `pagelink`; outputs `target/orpheus/firmware.yfw` (partition image), `resident.bin`, `pages.bin`, `firmware.map`, `layout.json` |
| `xtask pagelink` | The two-pass link and page packing (02 §"Demand paging") |
| `xtask size [--check]` | Size report: resident text/rodata/data/bss per crate, paged code/rodata per crate, RAM map by region and by mode pool; `--check` fails when a budget in `budgets.toml` is exceeded |
| `xtask check-resident` | Static proof that nothing reachable from an interrupt or fault handler lives in a paged section (§4.3) |
| `xtask sim [--script file] [--shot dir]` | Run the desktop simulator, optionally scripted and headless |
| `xtask test [host\|golden\|codecs\|fuzz-smoke\|all]` | Host test suites (§3) |
| `xtask test fuzz-seeds` | Rewrite the committed seed inputs of every fuzz target (§3.4) |
| `xtask test asan [--fuzz-only] [--lean]` | Host tests and every fuzz target's corpus under AddressSanitizer + LeakSanitizer (§3.5) |
| `xtask test miri [--crate NAME]` | Miri over the crate list in `xtask/src/sanitize.rs` (§3.5) |
| `xtask cover [--crate NAME] [--fuzz\|--fuzz-only] [--html] [--clean] [--report-only] [--add]` | LLVM source-based coverage (line, region, branch) of `crates/`, `sim/`, `xtask/`; `--fuzz` adds the fuzz-corpus replays, `--fuzz-only` runs them alone; reports in `target/orpheus/coverage/` (§3.6) |
| `xtask metrics [--elf PATH]` | Append this commit's coverage and static firmware rows to `metrics/*.csv` (§4.4) |
| `xtask gate [--no-dev] [--coverage] [--m11]` | The milestone gate (§6) |
| `xtask brand [--check]` | Brand assets (09 §6): palette contrast for the web and the RGB565 screen, lyre variants, favicons, app icons, social card, CSS custom properties into `docs/brand/generated/`; `--check` fails if they are not current |
| `xtask site [--check]`, `xtask site shots [--check]` | The website (09 §3) into `target/orpheus/site/` from `site/` and the Markdown in `docs/user`, `docs/dev`; `--check` fails on a broken internal link or fragment, a screenshot without a golden or out of date in `docs/shots/`, a manual page over 600 words, Home over 500 KB. `site shots` rewrites `docs/shots/` (also run by `test golden --bless`) |
| `xtask bringup <name>` | Build one bring-up image (small RAM-only binary under `bringup/`) |
| `xtask dev state` *(dev)* | Which mode the player is in: ADFU ROM, ymon, Orpheus (any personality), Rockbox, stock; battery voltage if readable |
| `xtask dev adfu` *(dev)* | Bring the player to ADFU + ymon from whatever is running (vendor SCSI reboot command for Rockbox/stock, EP0 request for Orpheus, nothing if already there) |
| `xtask dev ram <image> [--test name] [--wait s]` *(dev)* | Load a resident-only image to `0x100000` through ymon and start it; with `--test`, arm test mode, wait for the self-reboot, collect the result block, log ring and screenshot |
| `xtask dev stage [--layout …]` *(dev)* | Write the firmware partition (and page store) to NOR with verify; refuses without verified backups; never touches sector 0 |
| `xtask dev install-boot --i-have-a-backup` *(dev)* | Install or update the boot stages; the only command that writes sector 0 |
| `xtask dev install-full --i-have-a-backup` *(dev)* | Switch to the full-flash layout (02 §2.4); user decision D-2 only |
| `xtask dev log [--follow]` *(dev)* | Fetch and decode the defmt ring (running firmware via EP0, or post-mortem via ADFU) |
| `xtask dev shot out.png` *(dev)* | Screenshot: EP0 frame-buffer read when Orpheus runs, RAM read at `0x158000` in ADFU |
| `xtask dev keys "ok wait:500 down down hold:menu"` *(dev)* | Inject key events into the running firmware |
| `xtask dev hil <suite>` *(dev)* | Run an on-device test suite end to end (§5) |
| `xtask dev prof [--faults\|--pc]` *(dev)* | Collect a page-fault or PC-sample profile and symbolise it; `--update-hot` rewrites `hot.toml` |
| `xtask dev peek/poke/dump` *(dev)* | Register and memory access for hardware discovery (ADFU/ymon only for poke) |
| `xtask dev restore rockbox\|vendor-boot` *(dev)* | Put the previous state back: re-stage the Rockbox port from `yoton/build/ym03-main`, or restore the vendor sector 0 from the backup |

`xtask` uses `nusb` (pure Rust, no libusb) for USB. The ADFU protocol and ymon
commands are documented in `yoton.py` and FINDINGS.md §3, §11.

## 3. Host-side testing (no device)

Design rule that makes this possible: **only `atj2157-hal`, `yoton-bsp` and the
`firmware` binary depend on the hardware.** Every other crate (`orpheus-fs`,
`orpheus-audio`, all codecs, `orpheus-ui`, `orpheus-apps`, `orpheus-usb` classes,
`orpheus-bt` protocol logic, `orpheus-otp`, `orpheus-pager`'s policy code) is
`#![no_std]` with a `std` test configuration and takes its I/O through traits
(`embedded-storage-async`-style block device, `embedded-io-async`, the service
traits in 05 §5.1, `embassy-usb-driver`).

### 3.1 Unit and property tests

`cargo test --workspace` on the host. Expectations per area:

| Area | Tests |
|---|---|
| File systems | Operate on image files created by `mkfs.vfat` (FAT12/16/32, several cluster sizes) and `mkfs.exfat`; after every mutating test the image must pass `fsck.vfat -n` / `fsck.exfat -n`; power-cut simulation (block device that stops accepting writes at a random point) must leave a mountable volume; directory with 5000 long Unicode names; file > 4 GiB on exFAT; fragmented-file extent mapping |
| Codecs | §3.3 |
| DSP | Biquad responses against analytic curves; resampler SNR; volume/dither bit-exactness; no overflow at full scale |
| UI | §3.2 |
| USB classes | Mock `embassy-usb-driver` (or the class logic factored behind a transport trait): descriptor bytes, SCSI command/response sequences for disk and CD-ROM, error/sense handling, HID report streams |
| Authenticator | RFC 4226 / RFC 6238 vectors, vault encryption round trip, PIN back-off schedule |
| Settings store | Wear-levelled KV on a simulated NOR (4 KiB erase, bit-clear-only programming), power-cut at every write step |
| Pager policy | The line cache / LRU / region allocation logic compiled for the host against a simulated MPU: random access traces must never map two pages onto one physical line, and must reproduce the fault counts of the reference trace |
| Step counter | Recorded accelerometer traces with labelled step counts |
| Protocol codecs | Bluetooth frames (whatever docs/hw/bluetooth.md specifies), SD response parsing (CID/CSD/SCR), ID3/Vorbis-comment/MP4 parsers; all parsers fuzzed (§3.4) |

**Regression tests for fixed bugs (owner's policy, 2026-11-25).** Every bug
fix lands with a host test `regress_<bug>` (doc comment: bug id or STATUS
entry, date, symptom) that fails on the code before the fix; the fix's
commit carries both. Where the bug lives in hardware code, the decision is
moved into a host crate and tested there (example: `mailbox::stale_test`
for the stale test request; `Library::sample_step` for the Shuffle-all
watchdog reset); behaviour only the player shows also gets an on-device
check (a suite journey, a `dev` tool such as `usb-cancel`, or a soak
replay in `tests/regress/soak/`). Fuzz and soak findings keep their inputs
in `tests/regress/` (§3.4, §5.10). `cargo test regress_` runs the set, and
the §6 gate runs it as part of `xtask test all`. `docs/regressions.md` lists
every fixed bug with its test or on-device check; a fix adds its row.

### 3.2 Simulator and golden images

`sim/` is a desktop binary that links the same `orpheus-ui` and `orpheus-apps`
crates as the firmware and provides host implementations of the service traits:

* display → an in-memory 128 × 128 RGB565 buffer; optional SDL2 window through
  `embedded-graphics-simulator` at 4× scale; PNG export;
* keys → keyboard in the window, or a script (`ok`, `down*5`, `hold:menu`,
  `wait:300`, `shot:name`, `expect-screen:NowPlaying`);
* storage → a host directory or a FAT/exFAT image file through `orpheus-fs`;
* audio → the real `orpheus-audio` pipeline and codecs writing to a WAV file or
  the sound card; timing simulated so buffer logic is exercised;
* time → `embassy-time` std driver with a controllable mock clock for tests;
* radio, Bluetooth, sensors, USB, power → scriptable fakes that produce the
  `ServiceEvent`s the UI must handle (station found, device connected, battery
  at 4 %, card removed…).

Golden-image tests: each scripted journey ends in `shot:` commands; the PNGs
are compared byte-for-byte with `sim/golden/*.png`. `xtask test golden
--bless` rewrites them; a human-readable contact sheet
(`target/orpheus/golden.html`) shows old/new/diff for review. **Every screen in
05-ui-apps has at least one golden image**, in the Dark theme and one in
Light, with Latin, Cyrillic and CJK sample text.

The worker must actually look at the rendered PNGs (the Read tool displays
images) when creating or changing a screen: alignment, clipping and contrast
problems are not visible in a diff of numbers.

### 3.3 Codec conformance

`tests/corpus/` is generated, not committed: `xtask test codecs --gen` builds
it with `ffmpeg`/`sox` from synthetic sources (sine sweeps, pink noise,
impulses, multitone, silence, a 20 s music-like mix) at every supported sample
rate, channel layout and a spread of bitrates and encoder modes (CBR/VBR/joint
stereo; FLAC levels 0–8 and block sizes; Vorbis q0–q10; Opus 16–256 kbit/s in
SILK, CELT and hybrid ranges; ADPCM variants; with and without tags, gapless
info and seek tables). Reference decodes come from `ffmpeg`.

| Check | Threshold |
|---|---|
| Lossless formats (WAV, FLAC, ALAC, APE, WavPack lossless) | Bit-exact; FLAC also verifies the MD5 in STREAMINFO |
| Lossy formats | RMS difference to the reference ≤ −80 dBFS for float-reference codecs (MP3: ISO compliance "limited accuracy" at minimum), no sample offset (gapless trims exact to the sample) |
| Seeking | After seeking to 20 random positions, output from the seek point matches the linear decode from the same frame boundary |
| Robustness | Truncated and bit-flipped files: decoder returns an error or resynchronises; never panics, never loops forever (iteration budget), never reads out of bounds (run under Miri for the small codecs, plain tests + fuzzing for the rest) |
| Memory | Each decoder runs inside its fixed arena size (04-audio table); the test harness arena fails the test on exhaustion and reports the high-water mark |
| Speed (host proxy) | Instruction-count proxy is not reliable; real numbers come from the on-device benchmark (§5.4) |

### 3.4 Fuzzing

`cargo fuzz` targets (host, nightly toolchain only for this job): every
container/tag parser, every decoder's frame entry, the FAT/exFAT mount and
directory iteration on mutated images, the SCSI command handler, image
decoders, `otpauth` import, the Bluetooth frame parser. `xtask test fuzz-smoke`
runs each target for 60 s as part of the milestone gate; findings are fixed and
kept as regression inputs under `tests/regress/`. **Every finding stays in the
corpus permanently** (owner's requirement): the minimised input is committed
with a test that replays it on every gate run, and the coverage-guided
corpora of all targets are kept in the repository (`tests/fuzz-corpus/<target>/`,
pruned only by `cargo fuzz cmin`, never by deleting crash inputs). In M11
(VER-4) every target runs until its coverage plateaus.

How it is done (VER-4):

* One cargo-fuzz project per crate (`crates/<crate>/fuzz/`, `sim/fuzz/`),
  each its own workspace (nightly only). The target bodies live in the
  project's `src/lib.rs`; `fuzz_targets/<target>.rs` only calls them, and
  the crate's stable `tests/fuzz_corpus.rs` includes the same file
  (`#[path]`) and replays `tests/regress/<target>/` and
  `tests/fuzz-corpus/<target>/` on every `cargo test` (shared helper
  `tests/replay.rs`; `orpheus-fs`: `tests/fuzz_smoke.rs`; the codec targets:
  `crates/codecs/conformance/tests/fuzz_corpus.rs`). Target names are unique
  across projects (they name the corpus directories).
* Each body asserts invariants, not only "no panic": round trips where an
  encoder exists (records, frames, queue files, URIs, base32), models
  (settings and vault contents across remounts and power cuts), bounds
  (rows and letter jumps inside listings, device access inside the medium,
  CSW fields, list and name lengths), safety rules (no restricted Bluetooth
  command, charge code never above the 200 mA code) and, for the UI,
  golden-consistent redraws (a full redraw equals the incremental frame).
* Seeds: `cargo xtask test fuzz-seeds` writes `seed-*` files from the
  crates' own encoders and test fixtures (`ORPHEUS_WRITE_SEEDS=1` in the
  `fuzz_corpus` tests) and, for the codec targets, from the generated
  conformance corpus (≤ 24 files per target, cut to 16 KiB). Dictionaries:
  `tests/fuzz-corpus/<target>.dict`. Files ≤ 64 KiB, all corpora together
  < 20 MB (a corpus over its share is re-merged with a smaller `-max_len`).
* `xtask test fuzz-smoke [--secs N]` runs every target of every project
  from the committed corpus and dictionary; new inputs go to
  `target/fuzz-corpus/<target>/` and are merged into the committed corpus
  with `cargo fuzz cmin`. A finding is minimised (`-minimize_crash=1`) into
  `tests/regress/<target>/` (one left open, which still fails, into
  `tests/regress-open/<target>/` until its fix moves it) and listed in the
  findings table
  (`docs/verification/baseline-2026-11.md` §8.4).

### 3.5 Sanitizers (M11, VER-5)

* Host tests and fuzz targets also run under **AddressSanitizer** (nightly,
  `RUSTFLAGS=-Zsanitizer=address`, `--target x86_64-unknown-linux-gnu`),
  LeakSanitizer and, where the toolchain supports it, UBSan for the C-ABI
  boundaries; `xtask test asan` runs the whole host workspace and every fuzz
  target's corpus under ASan.
* **Miri** (`cargo +nightly miri test`) for every host-compilable crate with
  `unsafe` (the pager core, buffer code), and for the pure crates as a second
  oracle on undefined behaviour.
* Zero findings is the requirement; each finding becomes a regression test.

How it is done (VER-5, `xtask/src/sanitize.rs`):

* Toolchain: `$ORPHEUS_NIGHTLY`, default `nightly-2026-11-05`, with the
  `rust-src` and `miri` components (`rustup component add miri rust-src
  --toolchain nightly-2026-11-05`). `cargo-fuzz` for the fuzz targets.
* `xtask test asan`: `cargo +nightly test --workspace --lib --bins --tests
  -Zbuild-std --target x86_64-unknown-linux-gnu` with
  `RUSTFLAGS=-Zsanitizer=address` in `target/asan/` (`-Zbuild-std` because
  rustc refuses to link sanitized code against the uninstrumented std).
  `ASAN_OPTIONS=detect_leaks=1:detect_stack_use_after_return=1:…`; LSan is
  part of ASan on x86_64 Linux. Doctests are not run. Then each cargo-fuzz
  target: `cargo fuzz run <t> <scratch> <corpus dirs…> -- -runs=0` (ASan is
  cargo-fuzz's default; every corpus input once, no fuzzing; corpus
  directories as in §3.6). Logs and `findings.txt` (each report with its
  test binary, last test started, or the fuzz artifact) in
  `target/orpheus/asan/`. `--fuzz-only` skips the workspace tests.
  `--lean` runs one package at a time without debug info and deletes each
  package's test binaries and scratch images after its run (peak ≈ 1 GB
  instead of several; for the nearly full disk). The ASan build uses
  `panic = "unwind"` (`CARGO_PROFILE_DEV_PANIC`): with the profile's
  `abort`, `-Zbuild-std` produces a std the test harness cannot link.
  UBSan is not used: Rust code has no C UB to check and the workspace has no
  C sources (libFuzzer's runtime is the only C++).
* `xtask test miri [--crate NAME]`: `cargo miri test -p <crate> --lib` for the
  crates listed in `MIRI` in `xtask/src/sanitize.rs`, each in a time box
  (300–600 s; on expiry the whole process group is killed), with isolation
  on; the table names every skipped test and why (so far: file access
  under isolation — `orpheus-host` `png_roundtrip`, `orpheus-meta`
  `corpus_files`; over 5–10 min under Miri — `orpheus-otp`
  `stepping_equals_one_shot`, `uri::tests::fuzz_smoke` and the nine
  `vault::tests` (each unlock runs the KDF), `orpheus-meta`
  `garbage_never_panics_or_spins`,
  `orpheus-settings` `power_cut_at_every_step`, `orpheus-usb`
  `iso_reads_across_runs_with_padding`; ASan runs all of them). A full run
  takes ≈ 35 min (`orpheus-bt` and `orpheus-ogg` ≈ 8 min each). Logs and `summary.txt` in `target/orpheus/miri/`. The list: the
  host-compilable crates with `unsafe` (`atj2157-pac`; `orpheus-host`, whose
  only `unsafe` is the SG_IO ioctl Miri cannot execute, so only its other
  unit tests run) and a sample of pure crates as the second oracle
  (`orpheus-pager`, `-proto`, `-otp`, `-settings`, `-input`, `-power`,
  `-sdcard`, `-usb`, `-bt`, `-ogg`, `-meta`, `-audio`). Integration tests
  (files on disk) and the codec/FS/image crates (minutes per test under Miri)
  are left to ASan; VER-5 grows the list as tests get faster.

### 3.6 Coverage (M11, VER-1/2/6)

* `xtask cover` uses LLVM source-based coverage (`-Cinstrument-coverage`,
  `llvm-profdata`, `llvm-cov`): line, region and branch coverage per crate and
  for the whole project; dependencies are excluded by path. Reports (HTML,
  LCOV, summary CSV) are archived per commit under `target/orpheus/coverage/`
  and the summary goes into the metrics history (§4.4).
* Sources: host unit/property/adversarial tests, the simulator and golden
  tests, fuzz-corpus replays (`xtask cover --fuzz`), and device coverage
  (§5.8) merged into one report.
* The gate requires **100 % of non-excluded regions**. Exclusions live in
  `coverage-exclusions.toml` with a reason each and are reviewed by the owner
  (a user decision); only code proven unreachable qualifies.
* The firmware's logic is made host-testable behind small HAL seams (traits or
  `cfg(test)` mocks) so that it is covered on the host first; the device runs
  cover what only the hardware can reach (drivers, interrupt paths, pager).

How it is done (VER-1, `xtask/src/cover.rs`):

* `xtask cover` builds the host workspace's lib, bin and integration tests
  (doctests are not run) with the nightly toolchain (§3.5; branch coverage,
  `-Zcoverage-options=branch`, is nightly-only) and `-Cinstrument-coverage`
  for `x86_64-unknown-linux-gnu` in `target/cover/` (no debug info), runs
  them, and uses the same toolchain's `llvm-profdata`/`llvm-cov`
  (`rustup component add llvm-tools --toolchain nightly-2026-11-05`).
  `--crate NAME` limits the run and the report to one package.
* `--fuzz-only` (VER-4): only the fuzz corpus replays below, no host tests
  (source `fuzz`: the fuzz coverage per crate); a following `--add` run of
  the host tests merges into it (source `host+fuzz`).
* `--fuzz`: every cargo-fuzz project (`crates/**/fuzz/`, `sim/fuzz/`) is built with the
  same flags plus `--cfg fuzzing` (plain `cargo build`; libFuzzer only runs
  the given files) in `target/cover/fuzz/`, and each target runs every file
  of `tests/fuzz-corpus/<t>/`, `<fuzz dir>/corpus/<t>/`,
  `tests/regress/<t>/` and `<fuzz dir>/regress/<t>/`. A target without
  corpus files is not run (libFuzzer would start fuzzing).
* What counts: `*.rs` under `crates/`, `sim/`, `xtask/` except `tests/`,
  `benches/`, `examples/` directories and `tests.rs` modules (the tests are
  the instrument, not the subject; inline `#[cfg(test)] mod tests { … }`
  blocks still count, and are covered by running) and, without `--fuzz`,
  the fuzz projects. Files in `fw/` are device code (§5.8). Files with a
  function body that no instrumented binary contains are listed as
  **without data** (`no-data.txt`) and fail the M11 gate like uncovered
  code (files of declarations, tables and trait signatures have no regions
  and are not listed). Code behind a cargo feature the tests do not enable
  (e.g. `orpheus-sim`'s `window`) shows up there.
* Not yet in the run: the codec conformance binary over `tests/corpus/`
  (`xtask test codecs`) and the RFC 8251 Opus vectors — the codec crates'
  numbers are unit tests only until `xtask cover` runs them too (VER-2).
* Numbers: lines are llvm-cov's per-file line summary; regions and branches
  are computed by `xtask` as the **union over all instantiations and
  binaries** of each source span (a region is covered if any test or fuzz
  binary executed it; a branch counts its two outcomes). Approved exclusions
  are subtracted from regions and branches (`regions_excluded`); their lines
  still count.
* Outputs in `target/orpheus/coverage/`: `summary.csv` (per crate, worst
  first, then `TOTAL`; columns `commit, source, crate, lines_total,
  lines_covered, line_pct, regions_total, regions_covered, regions_excluded,
  region_pct, branches_total, branches_covered, branch_pct,
  files_without_data`), `files.csv`, `functions.csv` (every function with an
  uncovered region), `no-data.txt`, `lcov.info`, `html/` (`--html`), and
  `history/<commit>/` (summary and file CSVs of each run). `--clean` deletes
  `target/cover/` afterwards (1.5 GB for the workspace, mostly test images
  in its `tmp/`). `--report-only` recomputes the reports from the last
  run's `merged.profdata` and `objects.txt` (after editing exclusions).
  `--add` merges this run (typically `--crate NAME`) into the last run's
  profile and binaries and reports everything — for re-running one failed
  suite, or covering one crate at a time on a small disk.
* `coverage-exclusions.toml`: `[[exclude]]` entries with `path` (from the
  repository root), optional `function` (substring of the demangled
  function path; without it the whole file), `reason` (the proof of
  unreachability) and `approved-by`. New entries carry `approved-by =
  "pending owner"` and are listed but **not applied** until the owner
  replaces it (D-9).

### 3.7 Adversarial inputs (M11, VER-3)

Every function that takes data from outside (card files, tags, pictures, USB
requests, SCSI commands, Bluetooth frames, settings records, NOR contents,
`otpauth` URIs, the developer channel) gets a table-driven adversarial suite:
truncation at every byte, every length field at 0 / max / max+1, corrupted
magic/version/CRC, deep or cyclic structures, hostile names, arenas one byte
too small, and requests in every order; plus `proptest` properties with
recorded seeds.

As built (VER-3, 2026-11-27): a `tests/adversarial.rs` in every crate
that reads outside data, from the shared `tests/adversarial.rs` (cuts,
damaged bytes, field edges, hostile names, `sweep` of the committed fuzz
seeds through the fuzz bodies, `config` for `proptest`: a fixed seed per
property, failures recorded in `tests/regress/proptest/`). Long runs are
`#[ignore]`d. Inventory, findings and what is not covered yet:
`docs/verification/baseline-2026-11.md` §9.

### 3.8 UI sequence fuzzing on the host

A fuzz target drives the simulator with arbitrary key/timing/event sequences
over every screen and app (oracles: no panic, invariants of the shell and
services, golden-consistent redraws); its corpus seeds the on-device fuzzer
(§5.7), and sequences found on either side are replayed on both.

## 4. Build-time guarantees

### 4.1 Lints and style

`cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`
with `clippy::pedantic` selectively enabled; `#![forbid(unsafe_code)]` in every
crate except `atj2157-pac`, `atj2157-hal`, `orpheus-pager`, `firmware` and the
boot stages, where each `unsafe` block carries a `// SAFETY:` line. No `alloc`
anywhere in firmware crates (the `firmware` binary does not define a global
allocator, so a dependency that needs one fails to link — keep it that way).
Panics: `panic = "abort"`; firmware crates use checked operations and return
errors; `xtask size` reports the number of panic sites (`core::panicking`
callers) in the resident image and the gate fails if it grows without an entry
in `docs/STATUS.md` explaining why.

### 4.2 Size and RAM budgets

`budgets.toml` (initial values in 02-architecture §"Memory budget") is enforced
by `xtask size --check`:

* resident image bytes (text + rodata + data);
* static RAM (`.bss` + `.data`) per region;
* each mode pool partition;
* stack: painted at boot, high-water mark reported by the HIL suite; the
  budget is the measured maximum + 25 %;
* total firmware partition size per layout.

A change that exceeds a budget must either shrink something or raise the
budget in the same commit with the measured numbers and the reason.

### 4.3 Resident-set proof

A page fault inside an interrupt handler stalls the system for the duration of
a NOR read; a page fault inside the pager, or with the NOR busy, is fatal. So
the set of functions and data reachable from exception vectors must be
resident. `xtask check-resident`:

1. takes the final ELF and `layout.json`;
2. disassembles the resident image (the `yaxpeax-arm` or `capstone` crate),
   builds the call graph from direct branches (`bl`, `b`, `b.w`, tail calls)
   and literal-pool/`movw+movt` address loads;
3. starting from every vector-table entry and every function registered in the
   `#[resident_root]` list (wakers, the executor's poll entry, time-driver
   callbacks, DMA completion handlers), walks the graph;
4. fails if any reachable code or referenced rodata lies in a paged section,
   printing the chain `vector → … → offender`;
5. lists indirect calls (`blx rN`) found in the walk so that a human or the
   worker can confirm their targets are covered by `#[resident_root]`.

The run-time counterpart: the fault handler records "fault taken while an
exception was active" with the stacked PC in the persistent trace (02
§"Faults"); the HIL gate requires that counter to be zero.

### 4.4 Memory and performance history (M11, VER-10)

Tracked per commit as thoroughly as coverage, in `metrics/*.csv` (one row per
commit and measurement), with thresholds in `budgets.toml` and the gate
failing on a regression that is not explained in the same commit:

* static: resident image, `.data`/`.bss` per crate, page counts, the 40
  largest stack frames (`sub sp` from the disassembly), task future sizes;
* dynamic (from HIL and fuzz sessions): main-stack high-water, arena
  high-water per codec and picture format, pager loads/s, executor idle %,
  RT-task time per block, decode µs per frame per codec at each clock level,
  DSP cost, UI frame time, boot time, card/USB throughput, battery drain per
  PWR-4 scenario;
* host: per-crate benchmark timings (`cargo bench`), simulator frame time.

The verification report (VER-11) plots the history.

`cargo xtask metrics [--elf PATH]` (host side, VER-1/VER-10) appends the
current commit's rows; rows already present for the same commit (and, for
coverage, the same source) are replaced, so re-running is harmless. Commit
ids are `git rev-parse --short=12`, with `-dirty` when tracked files differed
from HEAD; `date` is HEAD's committer date. Files (committed, one header
line, comma-separated, text fields quoted when they may contain commas):

* `metrics/coverage.csv` — from the last `xtask cover`
  (`target/orpheus/coverage/summary.csv`), one row per crate plus `TOTAL`:
  `commit, date, source, crate, lines_total, lines_covered, regions_total,
  regions_covered, regions_excluded, branches_total, branches_covered,
  files_without_data`. `source` is `host`, `fuzz` (`--fuzz-only`) or `host+fuzz` (later
  `+device`, VER-6). `commit` is the commit the coverage run measured.
* `metrics/static.csv` — from the last `xtask build`
  (`target/orpheus/firmware.elf` and `layout.json`, or `--elf`), long
  format: `commit, date, build_id, metric, key, value` with metrics
  `resident_bytes`, `bss_bytes`, `stack_bytes` (the `.stack` section),
  `panic_sites`, `region_bytes` (key: region), `code_pages`, `rodata_pages`,
  `slot_bytes` (key: layout) and 40 × `stack_frame` (key: demangled
  function, value: bytes its prologue reserves — `push`/`push.w`/`stmdb
  sp!` 4 B per register, `vpush` 4 B per `s` and 8 B per `d` register, plus
  `sub sp, #imm` in the first 16 instructions, from `llvm-objdump -d`),
  and `stack_static` (key: each `stack.toml` context, `nested`, `usable`:
  the static worst case of `xtask stack`, below; needs the link map next
  to the ELF). `build_id` is the layout's, `-` without a layout.

Dynamic metrics (HIL, fuzz sessions, benchmarks) get their own files under
`metrics/` with their packages (VER-10 device side).

#### Static worst-case stack depth (`xtask stack`, VER-10, as built)

`cargo xtask stack [--check] [--top N] [--root PATTERN] [--elf PATH]
[--map PATH]` (`xtask/src/stack.rs`, configuration in `stack.toml`):

* **Graph**: the final link's reference graph (`linkgraph`, as for
  `check-resident`), restricted to direct branches between code sections.
  `BL`/`BLX` relocations are calls (the caller's frame stays live), `B`/
  `B<cond>` to another section are tail calls (frame popped first; the
  callee's depth replaces the caller's). A `bl` to the function's own
  start (no relocation within a section) is direct recursion.
* **Frames**: per function, the deepest its own SP adjustments go in
  address order (`metrics::parse_functions`: `push`, `str rX, [sp,
  #-4]!`, `vpush`, `sub sp` down; `pop`, `vpop`, `add sp`, `ldm sp!`,
  `ldr rX, [sp], #4` up; `add r7, sp, #n` / `sub sp, r7, #n` tracked so
  epilogues restore exactly). This catches shrink-wrapped prologues and
  second prologue copies the 16-instruction `stack_frame` metric misses.
  The DWARF CFI is no help: after the prologue it follows `r7`.
* **Contexts** (`stack.toml [[context]]`): where the stack is entered and
  at which priority — thread mode (`MainExecutor::run` with `main`'s frame
  below it; `main` before it starts the executors is the separate `init`
  phase), NVIC level 3 (USB), 2 (the RT executor and its tasks), 1 (time
  driver, DMA completions), 0 (the pager's MemManage handler; the T2
  profiler), and the fault shim, which continues on the 512 B emergency
  stack (checked against that size; the main stack takes its frame only).
* **Indirect calls** (`blx rN`, `bx rN`, `ldr pc`): targets per calling
  function (`[[indirect]]`, global or per context: the executors' task
  polls, `irq_dispatch`'s bound handlers, waker vtable slots, `dyn
  ByteSource`/`SeekSource`/`RowSink`/`ListSource`/`Handler`/
  `RequestHandler`, `fn` comparators), or a byte bound with a reason (and
  `absent` patterns that must stay unmatched). An unresolved site is
  listed with the instruction that loaded its register and makes every
  depth through it a lower bound (`>=`).
* **Recursion**: cycles are reported; `[[recursion]]` marks a cycle known
  to be bounded, counted as one more pass, otherwise a lower bound.
* **Nesting**, per phase: the deepest context of each priority stacked on
  the ones below, plus 108 B per exception (the eagerly stacked FPU frame,
  26 words, + 4 B alignment), plus one more frame for a fault. Compared
  with the stack above the MPU guard (`__stack_top` − (`__stack_bottom`
  rounded up to 256) − 256: what `rt::stack_unused` measures from).
* `--check` fails when the margin is below `budgets.toml [firmware]
  stack_static_margin_bytes`, a `stack.toml` pattern matches nothing, a
  task poll is in no context, an indirect site is unresolved, a cycle is
  unbounded, the emergency stack overflows, or a `hot.toml` cluster with
  `from` outgrows its `budget_lines` (§5.6). It is a gate step (§6).
* `--root PATTERN`: the depth, the worst chain and the deepest callees of
  any function (for finding what to cut).
* Host tests: synthetic graphs (calls vs tail calls, resolved, bounded and
  unresolved indirect sites, recursion, nesting) and an objdump listing
  (shrink-wrapping, second prologue, frame-pointer epilogue, self-calls).

Findings and the numbers per context: `docs/STATUS.md` (2026-11-25,
VER-10). The ways to cut depth that worked: a non-inlined function for
what builds a large temporary next to a deep call (`Shell::unentered` +
`enter_root`, `Shell::replace_top`, `spin_in`, `io_session`,
`derive_start`/`derive_finish`, HMAC per algorithm, the Vorbis set-up and
audio packet), a future polled in its own frame (`firmware::frame::Own`),
and a panic handler that cannot panic.

The second round (2026-11-25, nested worst case 21,812 → 16,188 B,
margin 204 → 6,340 B) found what made the card task's poll frames large:
**values that move by value through an `async fn`**. A large value
returned from an awaited future (the poll's `Poll<T>` output), passed by
value into a child future, or moved out of a `Result` is copied through
the poll function's frame, often twice, and that frame is static under
every chain of the function. The volume (736 B: out of `start`, into and
out of `release`) made the task's poll 2,328 B; the opened decoder
(976 B, out of `orpheus_codecs::open`'s `Result`) made `play_file`'s
2,320 B; the card (608 B) and volume made `start`'s 2,392 B. The fixes:
keep the value where it was created and use it by reference (`vol` an
`Option<Volume>` in the task's future, `start`/`release`/`open_card`
filling the slot; `let mut opened = open(…); let Ok((dec, info)) = &mut
opened`), and put each child of a loop that awaits many different
children (`idle`, `start`, `load_state`) behind `Apart` so its poll's
temporaries are in a frame of its own. Where a moved value cannot be
avoided, it goes in a future that is not above the deep chains
(`open_card`). Finding them: the `subw sp` of the poll and the
`__aeabi_memcpy` sizes around its calls in `llvm-objdump -d`; future
sizes come from the DWARF (`{async_fn_env#0}` structure sizes). The
same moves shrink the futures: the audio task's pool went 7,336 →
6,824 B.

## 5. On-device testing (hardware in the loop)

### 5.1 Transport and control without keys

| State of the player | How the host talks to it |
|---|---|
| Boot ROM ADFU (`10d6:10d6`) | Native ADFU commands; upload `ymon`, then full memory access |
| Orpheus running, any USB personality | **Developer channel on endpoint 0** (vendor requests, 06 §"Developer channel"): log ring read, frame-buffer read, key injection, settings/stat queries, test-command mailbox, reboot (normal, ADFU, stock) |
| Orpheus running, USB personality with no host function or hung | Watchdog → boot counter → ADFU after 3 failed starts (boot stage 2); then post-mortem reads |
| Rockbox / stock mass storage | Vendor SCSI `CC` identify + `CB 21` → reboots into ADFU |
| Powered off | Needs a human (Power key) unless wake-on-VBUS is confirmed (01-hardware unknown U-PMU-3). **Automated tests must never power the player off** until that is known; use reboot instead |

After any reset, RAM in SRAM0/SRAM1/SRAM3 keeps its contents (the boot ROM and
the stub touch only `0x100000–0x102800` and the top words of SRAM0). The
harness relies on this: the **result block**, the **log ring** and the **frame
buffer** are read back through ADFU after the firmware under test has rebooted
itself.

### 5.2 Persistent areas (fixed addresses, shared by firmware and host)

Defined once in the `orpheus-proto` crate (used by firmware, boot stages and
xtask):

| Area | Address | Content |
|---|---|---|
| Boot mailbox | `0x137FC0–0x137FFF` | Reboot reason/request ("ADFU", "STOCK", "TEST"), boot-attempt counter, test-mode word and test arguments. Stage 2 reads it before anything else. Layout kept compatible with the existing stub's `0x137FF4` ("ADFU") and `0x137FF8` (counter) words so either stub understands both firmwares |
| Result block | `0x137E00–0x137FBF` | `"YRES"` magic, test name hash, verdict, up to 64 counters, 128-byte message |
| Fault record | `0x137D00–0x137DFF` | Last fault: kind, PC, LR, CFSR, HFSR, MMFAR/BFAR, active exception, pager state, build id |
| Log ring | 2 KiB at the top of the SRAM3 RAM half | defmt frames, write index, wrap count, build id |
| Frame buffer | `0x158000–0x15FFFF` | RGB565, row-major |

(The exact addresses are fixed in WP-T-1 after the memory map in 02 is
implemented; the table gives the intended placement.)

### 5.3 Test-mode flow

```
xtask dev hil <suite>
  for each test:
    dev adfu                      # ROM + ymon
    (if the build changed) dev stage | dev ram
    write boot mailbox: TEST, test id, args, "return to ADFU on exit"
    start (jump to image, or watchdog reset to boot from NOR)
    wait for the device to re-enumerate as ADFU (timeout per test)
    read result block, fault record, log ring, frame buffer
    decode, compare, archive under target/orpheus/hil/<suite>/<test>/
```

In test mode the firmware runs the named test task instead of (or beside) the
normal UI, writes the result block, and reboots to ADFU. Tests can also be
driven live through the developer channel when the firmware stays up (key
scripts, screenshots, statistics polling); live mode is preferred once the USB
driver exists because it is much faster than reboot cycles.

### 5.4 Suites

| Suite | Content | Pass criteria |
|---|---|---|
| `smoke` | Boot to Home, screenshot, key round trip, log fetch, reboot | Screenshot equals the simulator golden image for Home (exact); no fault record; boot time reported |
| `drivers` | One test per driver: timer accuracy vs host clock, LCD pattern + read-back of panel ID, key ADC idle values, SD init + CMD18/CMD25 pattern test on a **scratch file region** (§7), NOR read/erase/program in the scratch sector, DAC DMA cadence (interrupt gap histogram), ADC capture statistics, I2C device scan, RTC set/read/alarm, PMU readings, UART loopback if pins allow | Per-test thresholds in the test source |
| `pager` | Fault-rate and latency statistics under scripted UI load; forced eviction storm; fault-in-exception counter | Zero fatal faults; counter zero; latency ≤ 300 µs per line |
| `audio` | Decode each corpus file from the card: PCM **CRC over the stream handed to the DAC DMA** compared with the host reference CRC; underrun counter; CPU % per clock level; gapless join check | CRC equal for lossless, tolerance check via captured PCM window for lossy; zero underruns incl. during scripted UI stress |
| `fs` | On-device create/write/read/rename/delete in `/.orpheus/test/`, then host-side `fsck` after switching to card-reader mode | Clean fsck, content hashes equal |
| `usb` | Enumerate each personality; MSC read/write speed and data integrity on the scratch area; CD-ROM image hash; HID reports captured on the host; suspend/resume; cable reconnect via soft detach | Hashes equal; no enumeration errors in the host's kernel log (`journalctl -k` snapshot before/after) |
| `power` | Clock-level sweep with a CPU benchmark; idle statistics (time in WFI); battery ADC logging; charger state transitions | Benchmark scales with clock; WFI share ≥ 95 % at idle Home screen |
| `soak` | 2 h playlist playback with periodic UI scripts and seeks | No fault, no underrun, no drift between played samples and wall clock beyond crystal tolerance |
| `soak-*` (VER-12, §5.10) | Several long mixed-use soaks: interactive browsing while listening to music and the radio, seeking, changing settings, more listening | No fault, no reboot, stack high-water above budget, no underruns beyond limits, pager and buffer statistics within budget, state consistent afterwards |

**Gapless join check** (part of `audio`, AU-5; runs without keys):

1. `cargo xtask test codecs --gen` makes `tests/corpus/gapless/{flac,wav,
   mp3,vorbis}_part{0,1,2}.*`: one 5-s source cut at frames 60,417 and
   131,333 (parts of 60,417 / 70,916 / 89,167 frames at 44.1 kHz). Upload
   each codec's three parts into `/.orpheus/test/gapless-<codec>/` with
   `cargo xtask dev put`.
2. `cargo xtask dev cmd "playdir /.orpheus/test/gapless-<codec>"` (folder
   order, shuffle off), then poll `cargo xtask dev cmd status` every 200 ms
   until it reads `idle`.
3. Pass:
   * `q=` goes 1/3 → 2/3 → 3/3, the titles are `<codec>_part0/1/2`, and
     `shuf=0`.
   * `underruns=0` on every poll. The counter is per track and covers that
     track's join.
   * `join: u=0 u_meta=0` after each join.
   * The final `idle frames=` is part 2's frame count.
   * MP3, FLAC and WAV must pass. Vorbis is reported (04 §2.2: < 30 ms
     inserted if not zero).
4. Reported in the run log: `join#N: head= open_us= loads= u= lib_us=
   art_us= u_meta= pf_us= prep_us= prep_loads=` per join (`-`: not
   measured yet; `prep_*`: the next stream probed and inspected before the
   join), and `ub: join= fill= serve= pf= prep= lib= art= dec=` (the
   track's underruns by cause, which add up to `underruns`). `open_us`
   must stay below the headroom (`head` frames at the stream's rate).
   Every `ub:` field must be 0.
5. A folder with a 44.1 kHz part followed by a 48 kHz file checks the
   rate-change fallback: the second file plays at the right pitch, after
   a short stop.

### 5.5 Audio verification without ears

1. **Digital tap**: in test builds the PCM sink computes a CRC-32 and a running
   sample count over exactly the bytes queued to the DAC DMA; the host has the
   reference decode. This proves the decoder + DSP + buffering path.
2. **DMA cadence**: interrupt timestamps prove the DAC consumes at the expected
   rate (catches clock-tree errors: a wrong divider shows as a wrong rate).
3. **Analogue loop, if the hardware allows**: record the DAC output through an
   internal analogue path into the ADC (docs/hw/audio.md says whether one
   exists) and check the tone frequency and level. If not available, analogue
   correctness (pops, hiss, channel swap, level) is a **user-assisted check**.
4. User-assisted checks are collected in `docs/USER-CHECKS.md` with exact
   instructions ("play `/.orpheus/test/lr.wav`; expect a 1 kHz tone on the left
   only, then the right only"). A worker adds entries; the user ticks them off.
   Milestone gates list which user checks they need.

### 5.6 Profiling

* **PC sampling**: a timer interrupt (1 kHz, hardware timer T1) stores the
  interrupted PC into a ring in spare RAM while a test runs; `xtask dev prof
  --pc` symbolises against the ELF and prints a flat profile by function and
  by crate, split into resident/paged.
* **Fault log**: the pager can log `{address, pc}` per fault (ring, test builds
  only); `xtask dev prof --faults` aggregates by section. `--update-hot`
  regenerates `hot.toml`, the ordered list of sections that `pagelink` packs
  together (browser path, directory scan path, each codec's frame loop, the
  Now Playing redraw path…), so working sets do not evict each other.
* **Stack**: painted at boot; high-water mark exposed through the developer
  channel and the Diagnostics screen.

#### As built (T-8; 2026-11-25, not yet run on the player)

* **Block**: one capture at a time in the 2 KiB **profiler block** at
  `0x307000` (SRAM3, after the PCM ring; `orpheus_proto::mem::SRAM3_PROF`).
  That place was reserved for the log ring, which runs in SRAM0
  (`logring::ALT_BASE`), so the block costs no RAM that anything else
  uses; SRAM0 gains 8 B of flags. Layout, entry formats and the thinning
  rule: `orpheus_proto::prof` (host-tested). The host reads it with PEEK
  (`REQ_PEEK`) after the capture stopped.
* **Thinning**: a capture started with `on` keeps a uniform sample of the
  whole run whatever its length: when the ring is full it keeps every
  other entry and from then on takes one event in two (the stride
  doubles; entry *i* is event *i* · 2^shift). `burst` stops at full
  instead (the first 500 samples or 250 faults, complete).
* **PC sampler**: timer **T2** (unused before; [DS] only, U-TMR-1) at
  1 MHz with the period given (default 1,000 µs), its own vector entry at
  **level 0** (not masked by BASEPRI: it sees critical sections and the
  level-1 handlers). A tick during a page fault is taken when the fault
  returns, on the faulting instruction, so paging time is charged to the
  code that faulted. One word per sample: PC with bit 0 = handler mode;
  500 entries.
* **Fault log**: `pager::orpheus_pager_fault` calls `prof::log_fault` for
  every fault served while the log is on (one load and one branch when
  off). Two words per fault: address, faulting PC, NOR read or re-map,
  data access or fetch (code / literal pool / rodata follows from the
  address), time since the start in 1,024 µs units; 250 entries.
* **Commands** (USB task, answered at once): `prof pc on|burst [µs]`,
  `prof faults on|burst`, `prof off`, `prof status` → `prof: faults on
  n=250/250 seen=41273 shift=8 period=0us t=30012ms at 0x307000`.
* **Cost**: resident +656 B (79,356 → 80,012 B: sampler entry 18 B and
  handler 74 B, `log_fault` 84 B, the shared ring push 310 B, the branch
  in the fault handler); `.bss` +8 B.
* **`cargo xtask dev prof`** `--pc S | --faults S` (or `--during
  "SUBCOMMAND …"`, run in the same process after the capture starts; a
  soak calls the profiler's hook after its prologue, around each action —
  `--only TEXT` captures only the first action whose text contains TEXT —
  and before its final reboot). It refuses a player whose build id differs
  from `target/orpheus/layout.json` (`--any-build`). Writes
  `target/orpheus/prof/<stamp>/`: `capture.bin`, `meta.json` (command,
  pager counters over the capture), `log.txt` (the log ring at the stop;
  `art: cover search` lines are printed), and `pc.txt`, `pc-functions.csv`,
  `pc-crates.csv` (flat profile by function and by crate, resident/paged ×
  thread/handler) or `faults.txt`, `faults-sections.csv`,
  `faults-functions.csv`, `faults-windows.csv` (counts by section with
  lines touched, kind and owning cluster; by faulting function; distinct
  lines per window, `--window-ms`; each cluster's share of the faults and
  its size against the 24-line budget). `cargo xtask prof DIR` reports a
  saved capture again (no player).
* **`--update-hot NAME`** (fault captures): the hottest paged sections by
  line loads, greedily within `--lines` (≤ 24, default 24) code lines and
  `--ro-lines` (default 8) rodata lines, skipping sections another cluster
  owns (pagelink gives a section to the first matching cluster) and names
  shared with them; hottest first. Replaces the `[[cluster]]` of that name
  in `hot.toml` (comments of other clusters kept) or appends it; also
  `hot-NAME.toml` in the report directory.
* **Clusters from the static call graph** (`cargo xtask hot`, AU-PERF
  2026-11-25; no player): for code whose faults a capture cannot single
  out (a chore while music plays: a game switch's capture is dominated by
  playback and UI faults). `hot [--name NAME] [--from PATTERN]… [--stop
  PATTERN]… [--lines N] [--fits-with A,B] [--top N] [--write]`: the call
  graph of `xtask stack` (direct branches, `stack.toml`'s global indirect
  targets), breadth-first from the `--from` globs, not entering `--stop`
  functions or the panic paths; reports the reachable functions, paged and
  resident bytes, paged bytes by crate and by owning cluster, the largest
  paged functions with their distance from a root; then picks the
  reachable paged functions closest to the roots first (ties: more
  callers among the reached), as literal name patterns, within `--lines`
  (default 8), skipping functions another cluster owns and names shared
  with one. `--write` replaces or appends the cluster in `hot.toml` with
  its `from`, `stop`, `budget_lines` (and `fits_with`, `starts_over`
  kept); without `--from` the stored roots are used, so a rebuild's
  cluster is regenerated with `hot --name NAME --write`.
* **Line budget check** (`xtask stack`, and `--check` fails on it): for
  each cluster with `from`, the reachable paged code (not entering
  `stop`) that is neither in the cluster nor in a `fits_with` cluster
  costs at least one line per started KiB of each function; the
  cluster's lines plus those must stay within `budget_lines`. A new
  function on the path, a cluster pattern that no longer matches or an
  inlining change that moves code out shows up as a build-gate failure
  instead of as `pl=` on the player. **Cache fit** (SYN-3, 2026-11-26):
  those lines plus the `fits_with` clusters' whole lines must also stay
  within the cache's 32 (`hot::CACHE_LINES`): before, the synth's render
  passed its 26-line budget at 25 while it ran with a 9-line cluster, 34
  lines in all, and paged 200–250 % of real time on the player
  (`regress_synth_working_set_over_the_cache`). The report line gives
  both (`… with its fits_with N of 32 cache lines`).
* **`starts_over = "NAME"`** (pagelink): a cluster starts on the physical
  line NAME starts on, the code window before it left empty (reported by
  `cargo xtask build`; 7 KiB for `flac-frame` after `games-io` and
  `dir-walk`). The playback clusters take all 32 physical lines,
  so a chore's cluster that runs with `play-fill` goes between
  `play-fill` and `flac-frame`, and `flac-frame` returns to `mp3-frame`'s
  lines this way.
* **What it cannot measure**: the profile is a sample (500 PCs / 250
  faults per capture: a function under ~1 % of the time or faults may not
  appear; distinct-line counts of a thinned capture are lower bounds, take
  a `burst` capture for a complete working set over ~0.1 s); time spent
  inside the pager's line loads is charged to the faulting PC, and MemManage
  itself is never sampled; samples are of T2's 1 MHz clock, not the core
  cycle count; with the core in WFI the sample is the idle loop; the
  fault log records the first fault of an access that straddles two lines
  and then the second, as the pager serves them; nothing survives a reboot
  (SRAM3 retention is not established), so a subcommand that reboots
  loses the capture unless it calls the hook.
* **Power proxy**: per-level time accounting in the clock governor (seconds
  spent at each frequency and in WFI) plus the battery-voltage log. Real
  current needs a meter: listed as a user-assisted measurement with a table to
  fill in (idle, MP3 screen off, FLAC screen off, radio, Bluetooth, recording).

### 5.7 On-device interaction and fuzz testing (M11, VER-7/8)

* **Functional suite**: one automated test per on-device function and per
  interaction of every screen and app (key, long press, repeat, every menu
  item, dialog and setting), driven by injected keys and developer-channel
  commands, with oracles: screenshot compared with the simulator in the same
  state, service state read back over the developer channel, audio CRC, and a
  diff of the test card image (writes only under `/.orpheus/`).
* **Fuzzed sequences**: `xtask dev fuzz-ui` sends random and model-guided key
  sequences (timing, long presses, repeats) interleaved with USB requests,
  card hand-over, playback, injected low-battery/clock/cable events; it
  checks for fault records, watchdog resets, stack-guard hits, the stack
  high-water budget, underruns, card writes outside `/.orpheus/`, and
  divergence from the simulator replaying the same sequence. Failures are
  minimised (delta debugging) and committed to `tests/regress/ui/` for both
  the HIL replay and the simulator.
* Sessions run on a dedicated test card image, never the owner's music card,
  and end in a reboot (§7).

#### As built (VER-7 tooling, VER-8 groundwork; 2026-11-24)

Host side in `xtask/src/dev/` (`script.rs`, `player.rs`, `mask.rs`,
`suite.rs`, `fuzz.rs`). Two firmware test commands
(`fw/firmware/src/ui.rs`, 9 bytes of RAM in all):

* `ui top` answers at once with `top ID NAME depth=D dialog=0|1
  overlay=0|1 locked=0|1 screen=0|1 look=0|1` (the UI task publishes its
  top screen after every loop).
* `ui test-look on|off` (reply `test-look on|off`): a RAM flag under which
  the appearance settings read as their defaults: Display 0x0304–0x030C
  (theme Dark, accent Blue, no wallpaper or screen saver, animations,
  no rotation, clock in the bar, all Home apps, lists roll round), the
  language (English), Now Playing's view and Power mode (Automatic). The
  settings store is not written; `off` and every reboot clear it; the UI
  reloads and redraws within its 1 s loop. These are the simulator's
  defaults (host test `simulator_starts_in_the_test_look`), so the owner's
  look (an accent, hidden Home apps) does not change the screenshots. The
  clock governor follows Automatic under the test look, a test session or
  the test mailbox whatever the owner's Power mode (PWR-5,
  `PowerMode::for_governor`), so a suite does not run at 24 or 288 MHz
  because of it.

**`cargo xtask dev suite [NAME…] [--list] [--update-expected] [--setup]
[--sim] [--no-reboot]`**

* Journeys: `tests/suite/journeys.toml`, scripts in the simulator's
  language, so one journey runs in the simulator and on the player. Lines
  of keys run on both; `sim:` lines only in the simulator (`set:`/`goto:`
  put the fake services where the player is); `dev:` lines only on the
  player; `cmd: TEXT ~ REGEX` sends a firmware test command and polls its
  reply (5 s) until it matches (the mailbox is primed with `ui top` first,
  so a stale reply never matches); a reply that does not match stops the
  journey (the steps after it rely on it: OK on Music's first row is
  Shuffle all when nothing plays), and a journey that stopped early sends
  `stop`. The Power key is refused everywhere (CLAUDE.md rule 4), and so
  are `set:`/`goto:`/`theme:`… outside `sim:` lines.
* Variants: a journey may list states of the player it accepts
  (`variants = [{ name, sim }]`, simulator commands run before it); each
  has its expected frames (`<shot>.<variant>.png`) and a shot passes if
  it matches one (the report names it, `shot@variant`). The owner's vault,
  the card's folders and the clock are never changed to suit a test:
  `authenticator` accepts the vault locked, absent (set-up note) or
  damaged; `usbdrive` the card with or without `/Images` and
  `/.orpheus/images`; `steps` the clock set or not. `appearance` (run
  before every reference journey) holds the player's fixed facts:
  `set:dual=0`, the full-flash layout.
* On the player: a tap is key down, 60 ms, key up, 40 ms (the simulator's
  timing); `expect-screen:` polls `ui top` (2 s); `shot:` takes the frame
  once two reads 100 ms apart agree and compares it with
  `tests/suite/expected/<journey>/<shot>.png`, the simulator's frame for
  the same journey (rewritten by `--update-expected`, or by
  `ORPHEUS_BLESS=1 cargo test -p xtask expected_frames`; a stale file stops
  the run). A shot that does not match is read again for up to 6 s (a
  list marquee starts 800 ms after the focus moves and rests 1 s per
  cycle; the player takes longer than 800 ms from the key to the shot);
  the closest frame is reported. A shot also fails when the player's top
  screen is not the simulator's.
* Masks: `tests/suite/masks.toml`, per screen name (`"*"` for all): the
  status bar (clock, battery, indicators), card listings, setting values
  (from x 48: a label is shortened to fit its value), Now Playing's art,
  text, progress and times (the play/pause icon is compared), the radio,
  the recorder's meter and card time, step counts. A shot can add
  rectangles, set `pixels`/`channel` tolerances, or drop the screen's
  masks (`defaults = false`, for menu sheets over masked lists).
* Every journey starts at Home: the screen is woken if off, the run stops
  if the keys are locked, a long Back, then Home's focus is found by
  matching the screenshot with the simulator's 11 Home frames and moved
  to the first app. It ends with a long Back that must reach Home (a short
  Back first if a dialog stays open). If no Home frame matches, the error
  names the closest one (focus, differing pixels, bounding box). Before the
  first journey the suite sends `stop` and `ui test-look on`; after the
  last it checks that the uptime did not go back and no new fault record
  appeared, sends `ui test-look off` and reboots the player (rule 4).
* `--setup` uploads the fixtures (two 30 s tones, WAV 44.1 kHz mono,
  generated by xtask, the same bytes every time; ≈ 100 s at 50 KB/s) to
  `/.orpheus/test/suite/music/` through `dev put`'s upload (only under
  `/.orpheus/test/suite/`, rule 5). `--sim` runs the suite against the
  simulator behind the same player interface (a dry run; the host test
  `suite_passes_on_the_simulator` does this on every `cargo test`).
* Report: `target/orpheus/suite/<UTC timestamp>/`: per shot the player's
  frame, the expected frame and a diff (magenta: differs, blue-grey:
  masked); `summary.txt`, `summary.json` (pass, differing/allowed/masked
  pixels, bounding box, command replies, seconds against the estimate) and
  `index.html`. The command exits non-zero on any failure.

Journeys in run order (estimate from keys, waits, 0.7 s per shot and
2.5 s for the start at Home; `--list` prints it):

| # | Journey | Content | Needs | Est. s |
|---|---|---|---|---|
| 1 | home | Home grid, app names in the strip, Home menu, quick panel | | 8.4 |
| 2 | settings | Settings root, first rows of all 12 groups | | 16.5 |
| 3 | system | About, Battery, Card info | | 9.7 |
| 4 | usb | Settings › USB, the When-connected choice (the USB dialog itself needs a cable event) | | 5.5 |
| 5 | files | Files at the card root, its menu | card | 4.7 |
| 6 | music | Music root (rows masked: Now playing, Resume, library), Folders (Up rolls round from the top) | card | 5.2 |
| 7 | nowplaying | `playdir` the fixtures, Now Playing, `status` underruns=0, menu, `stop` | `--setup` | 6.9 |
| 8 | pictures | Pictures at the card root, its menu | card | 5.1 |
| 9 | bluetooth | Bluetooth app as it is (never switched, rule 6) | | 4.6 |
| 10 | radio | Radio open, `fm status`, menu, long OK (radio off) | | 9.9 |
| 11 | steps | Today, Week, menu (variants: clock set or not) | | 5.9 |
| 12 | tools | Stopwatch start/stop/reset, Timer, Alarm | | 9.1 |
| 13 | recorder | Recorder ready and its menu (never records: `/Recordings` is outside `/.orpheus/`) | | 5.2 |
| 14 | authenticator | The vault as it is: PIN pad, set-up note or damaged (variants; no PIN, no secrets) | | 4.4 |
| 15 | usbdrive | USB Drive home (variants: `/Images`, test images present or not) | | 4.5 |

About 106 s in all, plus the reboot at the end (≈ 10 s).

Preconditions: firmware with `ui test-look` (it supplies Dark, Blue,
English, all Home apps for the session) and with Card info (VER-7 fixes,
2026-11-25), `--setup` run once with the 30 s fixtures, Bluetooth off,
the first entry at the card root a folder, nothing playing. The vault,
the clock and the card's image folders may be in any of their variants.

**`cargo xtask dev fuzz-ui --seed N --minutes M [--check-every K] [--play]
[--settings] [--no-minimise] [--max-replays R]`; `--replay FILE`**

* Keys from the `ui_keys` grammar (`sim/fuzz`: 3-byte operations; runs
  taken from the committed corpus alternate with fresh random ones, seeded
  by `--seed`): taps, holds (≤ 2 s), presses/releases, waits (≤ 2 s), long
  Back. No Power, no `set:`/`goto:`; `press:ok` becomes a tap and
  `release:ok` is dropped. A host test pins the grammar to `sim/fuzz`.
* Guard: `ui top` before every key. In a dialog every key becomes Back
  (delete, format, factory reset, restart, recovery and wipe all ask
  first); OK becomes Back on UsbConnect and QuickPanel (there also
  Prev/Next: Lock needs Power to undo), the Bluetooth screens (rule 6),
  Recorder (it would write `/Recordings`), Authenticator (set-up with an
  unknown PIN, back-off), HomeApps, and Settings unless `--settings`.
  Locked keys stop the session. The session runs under `ui test-look on`
  (off at the end; a crash's reboot clears it, so replays after a reset
  run in the owner's look). On the simulator, 3 seeds × 400 keys make
  no power, storage, USB-mode or factory-reset request (host test).
* Oracles after every K keys (default 20): fault record count unchanged;
  uptime monotonic; stack free ≥ `budgets.toml [hil]
  stack_free_min_bytes` (2048); `status` underruns not rising while
  playing (`--play` starts the fixtures). At the end the card's root
  listing (`ls /`, `/.orpheus/` left out) must be unchanged; then a reboot.
* A finding is replayed from Home (after a reboot for stack findings):
  first the last K keys, doubling until it reproduces, then delta
  debugging (ddmin), at most R replays (default 40). The result goes to
  `tests/regress/ui-device/<oracle>-seed<N>-<len>.keys` in the simulator's
  language: `--replay` runs it on the player, and every `cargo test` runs
  the directory in the simulator. The whole session is kept in
  `target/orpheus/fuzz/session-seed<N>.keys`.
* Not yet (VER-8 proper): model-guided sequences, interleaved USB
  requests, card hand-over, injected battery/clock/cable events, a
  test-card image diff (the root listing stands in for it), comparing every
  step with the simulator replaying the same keys.

First run: build and stage firmware with `ui top`, then `cargo xtask dev
suite --setup` (all journeys; the fixtures are uploaded once), open
`target/orpheus/suite/<timestamp>/index.html`, adjust masks or journeys
where the player's own state shows, run `cargo xtask dev suite` again.
Then, on a test card, `cargo xtask dev fuzz-ui --seed 1 --minutes 10`.

### 5.8 Device coverage (M11, VER-6)

Instrumented firmware builds carry LLVM coverage counters and a minimal
`no_std` profile runtime; after a HIL or fuzz session the counters are read
over the developer channel and merged on the host with the host coverage.
Counters cost RAM, so the instrumentation is applied to one crate or module
group per build (selected by a build flag) and the runs are merged.

### 5.9 Sanitizers for device code

Device code is checked for memory errors where it can run on the host (logic
behind the HAL seams, under ASan and Miri, §3.5); on the device the stack
guard, the MPU deny regions, fault records and the per-session stack
high-water play that role, and every fuzz session reports them.

### 5.10 Long soaks (M11, VER-12; owner's requirement 2026-11-24)

Several long soaks with real interactivity, designed to elicit stack
exhaustion, buffer under/overruns, paging thrash, leaks of state and
timing bugs that short tests never reach. They are part of the same
verification infrastructure as the unit, adversarial, ASan and fuzz tests
(§3.4–3.8) and the benchmarks (§4.4), and run on the reference player (and
the scenario logic also in the simulator).

* **Scenarios** (`cargo xtask dev soak <scenario> [--hours H] [--seed N]`),
  each a scripted, seeded loop that mixes:
  * `music`: album and shuffle playback across all codecs on the card,
    gapless joins, seeks (forward, back, to start, near the end), pause /
    resume, Next/Prev bursts, volume ramps, DSP on/off and EQ changes;
  * `radio`: radio on, seek and scan, preset changes, mute, switching
    between radio and music, recording from the radio;
  * `browse-while-playing`: Files and library lists scrolled hard (held
    keys, letter jumps, roll-around), pictures opened, Now Playing views,
    menus and dialogs, all while music plays;
  * `settings-churn`: settings changed while playing (theme, accent,
    language, sound settings, roll-around, Bluetooth on/
    off), with listening periods in between;
  * `mixed`: all of the above interleaved at random with long listening
    stretches (minutes) and idle periods (screen off, auto-off suppressed),
    plus card hand-over and USB personality switches;
  * `stress`: the above at maximum key rate and with the clock at its lowest
    allowed level, to push stack, pager and buffers to their limits.
* **Durations**: 2 h each for `music`, `radio`, `browse-while-playing`,
  `settings-churn`; 8 h and 24 h for `mixed`; 1 h for `stress`. Each runs
  on the plugged-in player; one `mixed` run also unplugged (battery) with
  the drain logger recording.
* **Oracles, sampled every few seconds and at the end**: no fault record,
  uptime monotonic (no watchdog reboot); main-stack high-water and task
  frame budgets (`budgets.toml [hil]`); underruns per cause (`ub:`) within
  limits and none outside seeks; pager loads/s and evictions within budget,
  no thrash episodes (loads/s spikes); PCM/stream ring and spill high-water;
  card I/O errors 0; settings store consistent and readable after the run;
  queue / resume / library intact; played-samples vs wall-clock drift within
  crystal tolerance; card root listing unchanged (writes only under
  `/.orpheus/`); a screenshot per checkpoint compared with the simulator
  where the state is deterministic.
* **Metrics**: every soak writes a time series (stack high-water, loads/s,
  underruns, ring fill, CPU idle, clock level, battery) into
  `metrics/soak/*.csv` and the VER-10 history; regressions against the last
  run fail the gate for release (REL-1: soak ×3).
* **Findings**: any failure is reduced to a minimal reproducing sequence and
  kept forever in `tests/regress/soak/` (replayed by the suite), like fuzz
  findings (VER-9).

#### As built (VER-12 tooling; 2026-11-24, not yet run on the player)

Host side `xtask/src/dev/soak.rs` (reuses the suite's Home frames, the
fuzzer's guard, `ddmin`, root listing and RNG, the player interface of
`player.rs`). Firmware test hooks (+22 B of statics; stack −48 B):

* `settings test-session on` → `test-session on held=N store=ok keys=K
  crc=C`: from now on setting changes (keys, `test-set`, radio presets,
  the radio's start station) stay in RAM and are **not written to NOR**;
  `held` counts them. No command switches it off: the reboot that ends
  every run does, and the stored values come back. `settings test-session`
  answers the same line; `keys`/`crc` describe the store as the last boot
  read it (CRC-32 over keys and values, and the presets).
* `settings test-set ID VALUE` (decimal or `0x…`), only in a session and
  only for Sound, Playback, Display (not Home apps), Language,
  Files sort/hidden and Now Playing's view (`ui.rs` `session_settable`,
  mirrored and checked by a host test): `test-set: queued`, then the UI
  task applies it as the screens do and answers `test-set ID=V ok|refused`.
* `ui screen-off`: the screen goes off without locking the keys (there is
  no backlight timeout yet; Power would lock them); the next key wakes it.
* `status` gains `pos=` (ms into the track); STATS group 0 gains word 21
  (the DAC headroom low-water in frames since the last read, reset by the
  read; `0x7FFFFFFF` none), 22 (failed card operations) and 23 (failed
  pager line loads). `xtask dev stats` prints them.

**`cargo xtask dev soak <scenario> [--hours H | --minutes M] [--seed N]
[--sim] [--report DIR] [--setup] [--battery] [--music DIR]…
[--no-minimise] [--max-replays R]`**, `--replay FILE [--sim]`, `--list`.

* Start: reboot, `ui test-look on`, `settings test-session on` (refused if
  the firmware lacks it), the store line, `stop`, the card's root listing,
  then the folders: `--music DIR` (repeatable) or found by a breadth-first
  `ls` walk (fixtures first, then `/` to depth 4, at most 40 listings, 16
  folders with ≥ 2 audio files, paths that fit the 120-byte command).
  `--setup` uploads 44.1/48 kHz tones and, if `xtask test codecs --gen`
  made them, the gapless parts per codec to `/.orpheus/test/soak/`.
  `--battery`: `charge pause` and `drain start soak stay` (the logger
  records, auto-off stays away; the cable stays in for the channel).
* Loop: actions from the seeded generator until the time is up (defaults:
  2 h, `mixed` 8 h, `stress` 1 h; `--sim` 10 min of simulator time); the
  oracles every `[soak] sample_every_s` (10 s), also inside long actions
  and key bursts. The first finding stops the run.
* End: keys released, Home, `rec stop`, `fm off`, `stop`, `gov on`; the
  card root listing compared; `queue.bin`/`player.bin` listed; the test
  look off; **reboot**; after it the store line must equal the one before
  the run (store mounted, same keys, same CRC: nothing persistent changed),
  and the state files must still be listed. Every error after the start
  reboots too, unless the player stopped answering (then rule 7).

Actions (one line each in a `.soak` file): `playdir PATH`; `seek next|prev
MS` (Now Playing, hold); `seek-start`; `pause MS`; `skip next|prev N`;
`volume N` (N × Vol+, then N × Vol−: never ends louder); `dsp on|off`;
`eq g0 g1 g2 g3 g4 bass crossfeed` (`sound`); `np-view N`; `listen S`;
`idle S` (`stop`, `ui screen-off`, wait, wake); `browse
files|library|pictures|settings|nowplaying STEPS SEED` and `menus STEPS
SEED` (seeded keys through the fuzzer's guard: taps, holds of 0.8–3 s,
letter jumps, OK, Back, Menu; stress: 20 ms press/release); `radio-on
[kHz]`, `radio-seek up|down`, `radio-scan` (session only: it replaces the
presets), `radio-off`, `radio-keys STEPS SEED` (Radio app: steps, presets,
mute pairs, seeks); `record S` (`rec start fm|mic adpcm S soak` →
`/.orpheus/test/rec/soak/`, ≤ 24 per run); `set ID VALUE`; `look
ID=V,… STEPS SEED S` (test look off, the owner's look plus these values,
keys where the player is, listening, test look on); `clock 48` (governor
off, Base) / `clock gov`; `handover S` (`usb msc ro`, S seconds, `usb dev`:
the computer gets the card read-only); `personality S` (`usb kbd`, back).
Never the Power key, never Bluetooth (`bt …`, its settings and screens are
guarded), never a writable card reader. Navigation: a long Back to Home,
the focus found by comparing the grid (status bar and strip masked, so a
playing mini-player does not matter) with the simulator's 11 Home frames,
then Next/Prev and OK; Now Playing through Music's first row (held Up
stops at the top). An app that does not open skips the action (counted);
five failed Home matches stop the run.

Weights (`--list` prints them; `soak.rs` `weights`):

| Scenario | Actions (weight) |
|---|---|
| `music` | playdir 8, shuffle (set 0x0201) 3, repeat 1, seek fwd 8, seek back 5, seek-start 3, seek to the end (6 s hold) 2, pause 5, skip 5, volume 4, dsp 3, eq 5, np-view 3, listen 60–300 s 20 |
| `radio` | radio-on 8, radio-seek 8, radio-scan 2, radio-keys 10, radio-off 4, playdir (music while the radio holds) 6, record 3, volume 3, listen 20 |
| `browse-while-playing` | playdir 4, browse files 20, library 15, pictures 10, nowplaying 6, settings 4, menus 8, seek 3, skip 3, listen 30–120 s 8 |
| `settings-churn` | look 15, set sound (EQ preset, balance, bass, crossfeed, channels, ReplayGain) 15, set playback (shuffle, repeat, gapless, seek step) 6, set display (brightness, animations, clock in bar) 6, dsp 3, eq 3, browse files 4, playdir 4, listen 60–240 s 25 |
| `mixed` | playdir 6, shuffle 2, seek 5+3, seek-start 2, seek-end 1, pause 3, skip 3, volume 3, dsp 2, eq 3, np-view 2, listen 10, long listen 300–900 s 6, idle 120–600 s 3, browse files 6, library 5, pictures 4, settings 2, nowplaying 2, menus 3, radio-on 3, radio-seek 2, radio-scan 1, radio-keys 3, radio-off 2, record 1, set sound 3, playback 1, display 1, look 3, handover 1, personality 1 |
| `stress` | first `clock 48`; playdir 4, seek 6+4, skip 6 (2–8 taps), pause 2, volume 3, eq 3, dsp 2, np-view 3, browse files 12, library 10, pictures 8, nowplaying 4, settings 3 (2–3× the steps, 20 ms keys), menus 6, radio-keys 2, set sound 3, look 3, listen 5–20 s 4 |

Oracles and limits (`budgets.toml [hil]`, `[soak]`; first values, to be
tightened from the first runs):

| Oracle | Limit |
|---|---|
| Fault record | none new |
| Uptime | never goes back (watchdog, reset) |
| Stack free (INFO) | ≥ 2,048 B |
| Underruns per track (`status`, by `ub:` cause) | 0 outside disruption windows; inside a window (10 s after a seek, skip, playdir, pause, idle, radio change, recording, clock change, card hand-over, personality, shuffle/repeat/Performance, or a browse that pressed OK; longer by the action's own time) ≤ 6 per window |
| Pager line loads/s | ≤ 7,000 for fewer than 3 samples in a row (thrash); run mean ≤ 4,500 (measured ≈ 2,670–3,220 in 48 kHz MP3) |
| Pager evictions/s | ≤ 7,000 for fewer than 3 samples in a row |
| DAC headroom low-water (STATS 21) | ≥ 1 frame outside windows (≤ 0: stale data played) |
| Card I/O errors (STATS 22), pager load errors (23) | 0 |
| DAC against the host clock (`played=` halves × 1,536 / rate over ≥ 300 s of unbroken playback) | ±2,500 ppm (crystal ±100 ppm, but U-AUD-10 measured +1,500 ppm) |
| Uptime against the host clock | ±300 ppm |
| Seek / pause (Now Playing, `pos=`) | moved the way the key asked; a note per miss, a finding after 3 misses in a row |
| Card root listing (without `/.orpheus/`) | unchanged |
| Settings store after the reboot | mounted, same keys, same CRC as before the run |
| `queue.bin`, `player.bin` | still listed after the reboot (their content is not read back: no test command reads them and the restored queue is published only when something plays) |
| `--battery` | stops (no finding) below 3,550 mV — reported as the battery minimum |

Not built: a screenshot per checkpoint compared with the simulator, the
stream ring and spill high-water (the PCM ring fill `written − played` is
recorded), regressions against the last run fail nothing yet (REL-1), CPU
idle has no limit. The **unplugged** `mixed` run cannot be driven from the
host (no cable, no channel): `--battery` keeps the cable for the channel
and pauses charging, which may or may not run the player from its cell
(open; PWR-5's user check 19 is the unplugged drain run).

Output: `target/orpheus/soak/<scenario>-<UTC>-seed<N>/` (or `--report`):
`samples.csv` (per sample: time, uptime, action, screen, playing, rate,
`pos`, queue, underruns and the eight causes, loads/s, evictions/s,
counters, idle %, clock level and switches, stack free, headroom, card and
pager errors, battery, PCM ring halves), `actions.soak` (the whole session,
replayable), `summary.txt`/`.json`, `index.html` (nine inline SVG charts
with the budgets dashed and findings marked; no scripts, no CDN). Player
runs also write `metrics/soak/<scenario>-<UTC>-seed<N>.csv` and a row in
`metrics/soak/history.csv` (the previous row of the scenario is printed).

Findings: replayed from a fresh boot each time (prologue as above): the
last 2, 4, 8 … actions until it reproduces, then `ddmin`, at most
`--max-replays` (8) replays; saved as `tests/regress/soak/<scenario>-
<oracle>-seed<N>-<len>.soak` (scenario, seed and finding in the header);
`--replay FILE` runs one on the player (or `--sim`); `cargo test` replays
every committed one in the simulator. After-run findings (card, store,
state files, drift, clock) keep the whole session.

`--sim`: the same tool against the simulator (`SoakSim`: the suite's
`SimPlayer` plus playback with a DAC model, radio, recorder, sessions,
test look, listings, personalities), in simulator time (30 min ≈ 16 s).
Host tests: generation deterministic per seed and every action round-trips
through its text; generated settings inside the firmware's whitelist and
valid; oracles pass a healthy synthetic stream and trip on a fault, a
reset, low stack, underruns, thrash, card errors, headroom, DAC drift and
clock drift; underrun windows; `ddmin` on actions; every scenario 40
actions in the simulator without a finding, Home found every time, no
power/storage/factory-reset request; a planted underrun found, replayed,
minimised and saved; a planted two-action bug minimised to exactly those
two.

First run (the lead): stage a firmware with these hooks; `cargo xtask dev
soak mixed --minutes 30 --setup --seed 1`; read `summary.txt` and
`index.html`; then the set: `music`, `radio`, `browse-while-playing`,
`settings-churn` (2 h each), `stress` (1 h), `mixed` (8 h, then 24 h), one
`mixed --battery`.

### 5.11 Unplugged measurement: audio quality, battery, power, performance (M13; owner's requirement 2026-11-25)

Everything before this measures the player **on USB**: charging, the host's
ground on the board, the developer channel awake. M13 measures it as it is
used — **on battery, USB disconnected** — with the analogue output recorded
by the workstation's microphone (or line) input, and optimises from the
numbers. Bluetooth may carry a low-rate status channel; the card carries
the full record.

**Rig and safety (MEAS-1).** The jack is direct drive (virtual-ground
return, §7): it may be connected to the workstation **only while USB is
disconnected** — then the player floats and the jack's sleeve is its only
ground. Procedure per run: arm over USB → unplug USB → plug the jack →
run → unplug the jack → plug USB. Firmware interlock while a run is armed:
VBUS appearing powers the PA and its virtual-ground buffer down at once
and ends the run (logged), so a cable mistake costs a run, not hardware.
Rig options are decision D-12 (manual swaps; an audio isolation
transformer, which makes USB + jack safe and also allows plugged/unplugged
comparisons; plus optionally a hub with per-port power switching,
`uhubctl`, so the host can cut VBUS itself for unattended runs). The
capture chain is characterised first (`xtask meas rig-check`): mono or
stereo input, plug-in-power bias (prefer a line input or a DC-blocking
adapter), sample rates, noise floor, clip level, frequency response
(workstation output → its own input loopback; through the transformer
when one is used); the result is a correction file every analysis applies,
and no figure is reported better than the chain can resolve.

*Reference workstation (owner, 2026-11-25):* ASRock Fatal1ty X370 Gaming K4,
Realtek ALC1220 (ALSA card `Generic`), **front-panel microphone input**:
stereo pin (node 0x19), capture 16/20/24-bit at 44.1–192 kHz, **mic bias on
(`VREF_80`, about 4 V through the bias resistor)**, `Front Mic Boost` 0 of 3.
Before measuring, the bias is switched off (pin control `VREF_HIZ`, e.g.
`hda-verb /dev/snd/hwC2D0 0x19 SET_PIN_WIDGET_CONTROL 0x20`, needs root;
or a DC-blocking adapter), boost stays 0 and the PipeWire source runs in
pro-audio mode at 96 or 192 kHz/24-bit. **Chosen (owner, 2026-11-25): the
rear blue line input** (node 0x1a, `Line In at Ext Rear`, already
`VREF_HIZ`: no bias, line level, `Line Boost` 0; ALSA `Input Source` set to
`Line` for the capture), so no root command is needed; the front mic input
stays the fallback.

**Reference player (owner, 2026-11-25).** A second, identical YM03 runs
the factory firmware. It is the reference for every comparison: the same
signal and music files are copied into a folder `ORPHEUS-REF/` on its card
(through its own card-reader mode, nothing else written), played through
its menus, and recorded the same way — through the workstation's line
input with its USB disconnected, and over Bluetooth A2DP into the
workstation (null-sink capture). Each AQ-1 figure is reported for both
players side by side; AU-0b (the periodic ticking, not heard with the
factory firmware) is checked on both under the same conditions. Its
battery runtime for the same files at the same volume is the PWR-6
baseline (started by hand from its menus; the logging is the
recording's length and its battery icon). **Its firmware is never changed
and no Orpheus tool talks to it** (CLAUDE.md rule 9): it is not connected
to the workstation's USB while any `xtask dev` command runs, and never put
into recovery mode.

**Run engine (MEAS-2, firmware).** A plan compiled by `xtask meas` into a
small binary file under `/.orpheus/meas/` (uploaded with `dev put`) and armed
over USB: steps such as play file/tone set at volume V, clock level or
governor, Power mode, screen on/off and backlight level, Bluetooth
on/off/streaming, radio, recording, pictures, idle, standby; each for a
time, a number of loops or until a voltage. The run starts a set delay
after VBUS goes (time to plug the jack), needs no keys, and stops at its
end, at the **voltage floor** (default 3.55 V; deeper only by D-13), on a
fault, or on a long Back press by the owner. Each step begins with an
**audio marker** (short chirp + FSK code: run, step, time) so the recording
aligns with the log without a shared clock. Logging extends the drain
logger: one row per 10 s per step (battery mV, its slope, clock level,
idle %, decode µs, pager loads, card I/O, underruns, DAC headroom,
backlight, Bluetooth state, faults) to `/.orpheus/meas/run-<n>/`. The run
ends idle with the output muted, screen off, waiting for USB; USB back
means a reboot (rule 4: never powered off, never left needing a key).

**Bluetooth status channel (MEAS-3, optional).** Within the commands this
project already uses (rule 6; no module firmware, D-6): compact status
lines (step, mV, underruns, faults) in the track-metadata fields of
command 0x31, read on the workstation from BlueZ (AVRCP `Track`
property), rate-limited after measuring what the module and BlueZ
accept; the A2DP stream itself captured as a second audio path (null-sink
method); the HID keyboard path only if BT-3 finds its feed. Bluetooth costs
power, so status-channel runs are separate from "dark" runs (card log
only), and its cost is itself a measured scenario.

**Host side (MEAS-4).** `xtask meas run PLAN` arms the player, prompts the
owner for each cable step (or switches the hub port, D-12), records the
input (PipeWire, 48 kHz/24-bit or the best the input offers), follows the
Bluetooth status, waits for USB, pulls the run directory, aligns by the
markers and analyses; `xtask meas report` writes an HTML report (discharge
curves, runtime per scenario, audio figures with plots) and appends
`metrics/meas/*.csv` (history, as VER-10). Thresholds live in
`budgets.toml` `[audio]` and `[battery]`; a regression against the last
accepted run is a finding.

**Audio figures (AQ-1).** From generated signal files on the card
(`/.orpheus/meas/signals/`, 44.1 and 48 kHz, 16/24-bit): frequency
response (log sweep), THD+N vs level and frequency (stepped sines, −1 to
−60 dBFS), dynamic range (AES17 −60 dBFS method), noise floor and idle
tones (digital silence, spectrum and a click detector — the periodic
ticking AU-0b, correlated with card, NOR paging, backlight PWM, charger and
display activity from the log), IMD (SMPTE, CCIF 19+20 kHz), crosstalk
(one channel driven; needs a stereo input or two passes), channel balance,
maximum level and clipping per volume step and PA gain, volume step
accuracy, pops at power-on, play, pause, stop, track change and volume
change, wow and flutter and clock drift (3,150 Hz tone; ppm against the
workstation), resampler aliasing (44.1 kHz content at 48 kHz), and the
DSP chain against its designed curves (EQ presets, bass, crossfeed,
ReplayGain). Each figure per state where it matters: unplugged vs plugged
(through the transformer or the A2DP path), screen on/off, clock level,
Bluetooth on/off.

**Battery and power (PWR-6, PWR-7).** Discharge curves from full to the
floor under several constant loads give the voltage-to-capacity model
(370 mAh cell) that the battery indicator and the low-battery warnings
use; runtime and average current per scenario (idle screen on/off, music
per codec with the screen off at listening level, radio, recording,
Bluetooth streaming, pictures, standby, off); Normal vs Maximum (D-4,
user check 19, automated here). Capacity cross-checked, if the owner has
a USB power meter, by the charge put back in. Optimisation is one candidate
at a time, the same plan before and after: deep idle and WFI, peripheral
clock gating, governor thresholds per codec, read-ahead size and card idle,
paging working set (NOR reads cost energy), screen-off rendering,
backlight curve, DAC/PA power-down on pause and stop (pop-free), Bluetooth
module and FM tuner off when unused, accelerometer low-power mode, polling
intervals. Regulator and charger fields stay at vendor values (rule 3).

**Performance on battery (PERF-1).** Decode headroom per codec and clock
level, pager load rate, UI frame time and key latency, underruns — over a
whole discharge, to catch behaviour that changes as the voltage falls
(Maximum near the floor in particular); results join VER-10's history.

### 5.12 Setting → effect coverage (M11, VER-13; owner's requirement 2026-11-25)

**Every setting, at every value, is shown to have its effect in every
context it applies to — global options in all contexts, local options in
their relevant contexts — and no effect anywhere else.** 100 % of the
matrix, measured like code coverage.

* **The manifest** `tests/settings/effects.toml` lists every setting of the
  settings tree (the tree in `orpheus-apps` is the source of truth, not a
  hand list): its scope (`global` or `local`), its values or range (with
  boundary and representative values for ranges), the **contexts** it
  applies to, and for each the **oracle** that shows the effect:
  * *global* settings (theme, accent, text size, language, list
    roll-around, clock in the status bar, volume limit, key click,
    transport keys when locked, auto-off, backlight and lock timers,
    Power mode, …) apply to **every context**: every screen and app of the
    screen registry, with the screen off, locked, during playback, radio,
    recording, the USB personalities and Bluetooth streaming, as far as
    the effect can show there;
  * *local* settings (Files sort and hidden files, Pictures fit and
    slideshow, Now Playing view, recorder format and source, radio region
    and step, game and Sequencer options, …) apply to their app's screens
    **and to every other context that uses them** (hidden files also in
    Pictures, the Library and the USB Drive picker; sort order also in
    playback queues made from a folder);
  * oracles: screenshot difference against the simulator (UI), the
    developer-channel state (`status`, `ui top`, STATS words), audio
    (rendered-block CRCs and spectra for sound settings and the DSP chain;
    the DAC volume and PA registers), the clock level and governor
    statistics, timers with injected time (auto-off, backlight, lock,
    sleep timer, alarms), USB enumeration (personality on connect), the
    Bluetooth module's commands, and the card (files written only where a
    setting says).
* **Differential tests**: for each setting × value × context, the same
  journey runs with the value under test and with a reference value; the
  oracle must differ in the declared way, and every other oracle must stay
  equal (catches settings that leak into contexts they should not touch).
  Interacting settings (volume limit × ReplayGain × EQ × bass × balance;
  theme × accent × text size; sort × hidden; Power mode × governor) are
  covered pairwise.
* **Persistence and defaults**: each setting survives a reboot and a
  power-off, a factory reset restores its default, an out-of-range stored
  value is clamped, and a store from an older firmware migrates.
* **Where it runs**: UI-visible effects in the simulator for every screen
  (fast, exhaustive); every effect that only hardware shows (sound, clock,
  backlight, timers, USB, Bluetooth, charging-independent ones) on the
  player through the test-session settings (`settings test-set`, RAM only
  — the owner's settings are never changed by a test), extended to the
  settings the whitelist does not yet allow, each with its safety reason.
* **Gate**: `cargo xtask settings-matrix --check` fails when a setting in
  the tree has no manifest entry, a declared context has no test, a value
  is never exercised, or an oracle did not change; it reports the matrix
  coverage (settings × values × contexts) next to code coverage in the
  verification report. A new setting lands with its manifest entry and
  tests in the same commit (CLAUDE.md).

**As built (VER-13, 2026-11-25).**

* `cargo xtask settings-matrix [--check] [--strict] [--manifest PATH]`
  (alias `cargo xtask settings coverage`; `xtask/src/settings_matrix.rs`)
  takes the settings from the code (`orpheus_apps::settings::all()` with
  `games`, `salvo`, `bricks` and `books` on: every item with a value,
  packed fields and their storage words; actions and information rows
  have none), parses the manifest as TOML (unknown fields refused) and
  checks: one entry per setting and none for a setting the tree lacks;
  scope, values (all choices, both ends of a range), the code's default,
  contexts, oracle and tests; every context declared in `[contexts]` (or
  an `@group` of `[groups]`); a global setting accounts for every context
  (affected, `unaffected`, or `na` with a reason); every context, affected
  or unaffected, covered by a named test or device check (`cover`) or
  listed in `uncovered` with the reason; at least one covered unaffected
  context; every test resolves to a `#[test]` fn of that module path in
  that file that is not `#[ignore]`d; every `suite:<journey>` exists; and
  `[kinds]` names persistence, default, out-of-range and old-store tests
  for each kind the tree has. It prints the table and the counts by scope
  (global, local, storage; device). `--check` fails on any error (part of
  `xtask test all`, so of the gate); `--strict` also fails on a reviewed
  exclusion (`gap`: a setting with no effect to show; `uncovered`: a
  declared context without a test) and is a step of `xtask gate --m11`.
* **The simulator matrix** (`sim/tests/settings_matrix.rs`, one test per
  setting, 7 s for all) reads the manifest: every non-default value against
  the default (`set:KEY=…` before the journey) in each of 59 screen
  contexts (`@screens`: the frame) and 11 service contexts (`@services`:
  the queue, a track's end in its middle and the queue's end, the tuner's
  register writes, the recorded file, the keyboard reports, the USB
  personality requested, the Bluetooth module, steps counted, an alarm
  ringing, the sleep timer, the volume reached, the key clicks the output
  got). The observation must differ exactly where `contexts` says and
  nowhere else; a journey that ends elsewhere (another app on Home,
  another file in a sorted folder) is an effect too. Effects that need a
  second setting, time or a file have dedicated tests named in `cover`
  (custom EQ bands with their preset, input gain with manual gain, the
  four alarms' time/days/on, the games' score-table variants after a
  finished game, list roll-around; VER-13's second round: Next held with
  each seek step, a start-up with a resume point, a podcast played again,
  the silence of Gapless off, clicks with the recorder listening).
* **Host contexts**: the sound (`sim/web` `Sound::from_settings` over the
  real `orpheus-dsp` chain: each Sound setting changes the rendered
  output, no other setting reaches the chain; the pause fade of the chain
  and the browser build, the key click), the governor (`orpheus-power`),
  the step detector (`orpheus-steps`), the recorder's split
  (`orpheus-rec`), the player's gapless and automatic-bookmark rules
  (`orpheus-player`).
* **Player**: `device = true` entries; `checks` name suite journeys that
  change a setting only through the RAM-only test session (the journey
  stops unless `settings test-session` answers "on", and sets the value
  back): `settings_shuffle` (status `shuf=` follows 0x0201),
  `settings_sound` (`clicks=` counts the key clicks sounded, `fade=` is
  the last pause's fade, 0 or 60 ms) and `settings_playback` (Next held
  with the 30 s step ends the 30 s fixture; `gapless=` says whether the
  last track change at a track's end was gapless). Effects the
  player alone shows and no read-only command reports yet (the applied
  Sound, the clock level, the ADC gain, auto-off, the step task, the
  library's code page) are `uncovered` with that reason.
* **Second round (2026-11-26)**: Key click, Fade on pause, Gapless,
  Resume at start-up, Seek step and Auto-bookmark long files have their
  effect (04 §2.2, 04 §5.1, 05 §3, §6.1, §7.1 "As built (VER-13)");
  16 settings remain `gap` (Speed: 04 §5.1's time stretch is not built,
  see its entry), 63 reviewed exclusions.
* **Display, Power, Date & time, USB (VER-13 follow-up, 2026-11-26).**
  Backlight timeout, Lock delay, Screen saver, Animations, Rotate, Power
  mode, Keys when locked, Time zone, Correction, Set from RDS, Read-only
  card and Developer channel have their effects (05 §6.3, 06 §2.5, §5.3).
  The simulator holds the screen timers by default, as the firmware does
  under the test look (`set:timers=run` runs them); the matrix compares
  what the panel shows (`Sim::panel`, turned when rotated) and, for
  `usb-host`, also `HostServices::usb_offer` (developer interface,
  write-protected card). Device journeys `settings_screen_timers` and
  `settings_screen_saver` read `ui top` (`screen=`, `locked=`, new
  `saver=`) through the RAM-only session; a session's `test-set` of 0x0302,
  0x0303 or 0x0307 off its default arms the timers under the look, setting
  it back holds them again with the screen on and the keys free. The
  session refuses the Developer channel (host test). Brightness (no stock
  duty values, U-PWM-3) and Wallpaper (Pictures › Set as wallpaper not
  built) stay `gap`.

## 6. Milestone gate (run before declaring any work package done)

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
xtask test all                 # host unit, golden, codecs, fuzz-smoke,
                               # settings-matrix --check (VER-13, §5.12)
xtask brand --check            # palette contrast, generated brand assets current
xtask site --check             # site builds; internal links, screenshots, page limits
xtask build --release          # both layouts once the full layout exists
xtask size --check
xtask check-resident
xtask stack --check            # static worst-case stack depth with nesting (§4.4)
xtask dev hil smoke            # plus the suites named by the work package
```

Coverage and sanitizer steps (VER-1, VER-5): `xtask gate --coverage` adds
`xtask cover` as a **report-only** step — it prints the numbers and fails only
if the coverage run itself fails, never for being below 100 %; that is the
behaviour until M11. `xtask gate --m11` (the M11 gate) adds `xtask cover
--fuzz` and **fails below 100 % of non-excluded regions** or when a project
source file has no coverage data, and adds `xtask test asan` and `xtask
test miri` (any finding fails). Without these flags the gate is unchanged.
After a gate run on a commit, `xtask metrics` records the numbers (§4.4).

Then update `docs/STATUS.md` (what was done, measured numbers, new unknowns,
user checks requested) and commit. A gate that cannot run (for example the
player is unplugged) is reported as **not run**, never as passed.

## 7. Safety rules for anything that touches the player

* **Analogue output measurements**: the headphone jack is direct drive (its
  return is a virtual ground). Never connect it to an input that shares
  ground with the USB host (a computer's line or microphone input): it
  shorts the virtual-ground amplifier and makes the player misbehave
  (observed 2026-11-24: card initialisation fails at every boot). Use an
  audio isolation transformer, or measure through the Bluetooth A2DP
  capture (07 §5.5, null-sink method in STATUS 2026-11-24). With **USB
  disconnected** (the player on battery, nothing else attached) the jack
  may go to the workstation's input directly: that is how M13 measures
  (§5.11, procedure and interlock there).

These are repeated in `CLAUDE.md`; they are here with their reasons.

1. **NOR sector 0 and the vendor area are off limits.** Only `xtask dev
   install-boot --i-have-a-backup` writes sector 0, and only bytes
   `0x000–0x1FF`. In the dual-boot layout nothing below `0x762000` is ever
   erased or programmed. Reason: the mask ROM boots from sector 0; the ROM
   falls back to USB recovery if the record is invalid, so the player is hard
   to brick, but a half-written vendor area cannot be re-created without the
   backup.
2. **Verify the backups before the first NOR write of a session** (`sha256sum`
   of `dumps/nor_backup_8m.bin` begins `ad45687e`). If the files are missing,
   stop and tell the user.
3. **Program-and-verify always**; a failed verify stops the command.
4. **Do not change regulator voltages, charger current or charger voltage
   beyond values the vendor firmware itself uses** (docs/hw/system.md lists
   them). A lithium cell is attached. Any experiment that writes a PMU
   regulator or charger field for the first time needs the value's provenance
   written in the commit message.
5. **Do not power the player off from an automated test** (see §5.1), and do
   not leave it in a state that needs a key press: every test ends in a reboot
   to ADFU or to the installed firmware.
6. **The card holds the owner's music.** Automated tests write only inside
   `/.orpheus/` (create it if needed) and never format, repartition, or run raw
   block writes outside files they created. Raw-block driver tests use the LBA
   range of a pre-allocated scratch file (`/.orpheus/test/scratch.bin`, located
   through the file system, extents verified contiguous). MSC write tests from
   the host go through the mounted file system into the same directory.
7. **Rockbox is the working reference on the device** until milestone M2
   replaces it in the free NOR region. Before that, run only RAM-loaded
   bring-up images. After that, `xtask dev restore rockbox` must work (test it
   once) so the reference can be brought back.
8. **One process talks to the player at a time.** Parallel workers must not
   share the device; hardware work packages are serialised (08-milestones).
9. If the player stops responding: wait for the watchdog (boot stage counts
   three failed starts, then enters ADFU); if it does not come back within two
   minutes, stop and ask the user to unplug, press the reset pinhole next to
   the earphone jack, and hold **Next** (stock "Right") while plugging in to
   force USB recovery. Do not keep retrying blind.

## 8. Tooling work packages

* **WP-T-1 Workspace and proto**: Cargo workspace, toolchain pin, lint config,
  `orpheus-proto` (addresses, mailbox, result block, developer-channel request
  numbers), `budgets.toml`, `xtask` skeleton with `build`, `size`, `test`.
* **WP-T-2 Host transport**: `orpheus-host` (ADFU, ymon protocol, NOR payload
  drivers, PNG), `xtask dev state/adfu/ram/peek/poke/dump/shot/backup-verify`.
  Acceptance: byte-identical NOR dump of the scratch sector vs `yoton.py`;
  screenshot of the running Rockbox frame buffer.
* **WP-T-3 Pagelink + check-resident** (with 02's pager work package).
* **WP-T-4 Simulator + golden harness** (with WP-UI-1).
* **WP-T-5 HIL harness**: test mode, result block, suites `smoke`, `drivers`.
* **WP-T-6 Rust payloads**: ymon, NOR programmer/dumper as `no_std` Rust
  binaries; remove the dependency on the C payloads.
* **WP-T-7 Codec corpus + conformance harness + fuzz targets.**
* **WP-T-8 Profilers and `hot.toml` loop.**
