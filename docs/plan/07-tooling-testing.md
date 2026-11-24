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
| `xtask test asan [--fuzz-only] [--lean]` | Host tests and every fuzz target's corpus under AddressSanitizer + LeakSanitizer (§3.5) |
| `xtask test miri [--crate NAME]` | Miri over the crate list in `xtask/src/sanitize.rs` (§3.5) |
| `xtask cover [--crate NAME] [--fuzz] [--html] [--clean] [--report-only] [--add]` | LLVM source-based coverage (line, region, branch) of `crates/`, `sim/`, `xtask/`; `--fuzz` adds the fuzz-corpus replays; reports in `target/orpheus/coverage/` (§3.6) |
| `xtask metrics [--elf PATH]` | Append this commit's coverage and static firmware rows to `metrics/*.csv` (§4.4) |
| `xtask gate [--no-dev] [--coverage] [--m11]` | The milestone gate (§6) |
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
* `--fuzz`: every cargo-fuzz project (`crates/**/fuzz/`) is built with the
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
  files_without_data`. `source` is `host` or `host+fuzz` (later
  `+device`, VER-6). `commit` is the commit the coverage run measured.
* `metrics/static.csv` — from the last `xtask build`
  (`target/orpheus/firmware.elf` and `layout.json`, or `--elf`), long
  format: `commit, date, build_id, metric, key, value` with metrics
  `resident_bytes`, `bss_bytes`, `stack_bytes` (the `.stack` section),
  `panic_sites`, `region_bytes` (key: region), `code_pages`, `rodata_pages`,
  `slot_bytes` (key: layout) and 40 × `stack_frame` (key: demangled
  function, value: bytes its prologue reserves — `push`/`push.w`/`stmdb
  sp!` 4 B per register, `vpush` 4 B per `s` and 8 B per `d` register, plus
  `sub sp, #imm` in the first 16 instructions, from `llvm-objdump -d`).
  `build_id` is the layout's, `-` without a layout.

Dynamic metrics (HIL, fuzz sessions, benchmarks) get their own files under
`metrics/` with their packages (VER-10 device side).

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

## 6. Milestone gate (run before declaring any work package done)

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
xtask test all                 # host unit, golden, codecs, fuzz-smoke
xtask build --release          # both layouts once the full layout exists
xtask size --check
xtask check-resident
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
