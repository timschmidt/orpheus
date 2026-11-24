# Getting started

From a fresh clone to the simulator in about 15 minutes, and to a first
change with a passing gate in about an hour. The deeper documents are the
plan ([`docs/PLAN.md`](../PLAN.md), [`docs/plan/`](../plan/)) and the
work log ([`docs/STATUS.md`](../STATUS.md)); this page links to them
instead of repeating them.

## 1. Prerequisites

* **Linux.** Development happens on Fedora; any recent distribution
  should do. macOS and Windows can run the host tests and the simulator,
  but the device tools are only tested on Linux.
* **rustup** ([rustup.rs](https://rustup.rs)). Nothing else: no C
  compiler, no libusb (USB is pure Rust through `nusb`).
* About **10 GB of free disk** for a full build of both workspaces.

The toolchain is **pinned** in [`rust-toolchain.toml`](../../rust-toolchain.toml)
(stable 1.98.1, the `thumbv7em-none-eabihf` target, rustfmt, clippy,
llvm-tools). `rustup toolchain install` in the tree installs it (recent
rustup versions also do it on the first `cargo` command). A nightly
is needed only for fuzzing, coverage and sanitizers (`ORPHEUS_NIGHTLY`,
see [07 §3.5](../plan/07-tooling-testing.md)).

## 2. Clone and build

```sh
git clone https://github.com/timschmidt/orpheus
cd orpheus
cargo test --workspace          # host crates, simulator, golden images
cargo xtask build               # firmware: target/orpheus/firmware.yfw
```

There are **two Cargo workspaces**. The root one holds everything that
runs on the desktop (`crates/`, `sim/`, `xtask/`); `fw/` holds the device
crates (HAL, board support, firmware, boot stage, payloads) and builds for
the Cortex-M4F. Shared `no_std` crates under `crates/` are used by both.

## 3. The `xtask` tour

Every task goes through one entry point, `cargo xtask <command>`; each
prints a one-line verdict last and fails with a non-zero exit code.
`cargo xtask help` lists them; the full table is
[07 §2](../plan/07-tooling-testing.md).

| Command | Does |
|---|---|
| `test host` / `test golden` / `test all` | Host test suites |
| `test golden --bless` | Rewrite the golden images after an intended change |
| `build`, `size --check`, `check-resident` | Firmware image, RAM/size budgets, the resident-code proof |
| `gate --no-dev` | Everything a change must pass, without the player |
| `brand`, `site` | Brand assets, the website and its screenshots |
| `dev …` | Talk to the player (below) |

## 4. The simulator

The simulator runs the real UI and app code against desktop back ends:

```sh
cargo run -p orpheus-sim -- --script "ok down*2 shot:music"   # writes target/orpheus/shots/music.png
cargo run -p orpheus-sim --features window -- --window          # interactive, 4x
```

In the window: arrows are Up/Down/Prev/Next, Enter is OK, Backspace is
Back, M is Menu, + and − are volume, P is Power, S takes a screenshot.
Scripts use the same key names plus `hold:KEY`, `wait:MS`,
`expect-screen:NAME` and `shot:NAME`.

## 5. Golden images

Every screen has golden images in [`sim/golden/`](../../sim/golden/)
(dark and light theme, Latin, Cyrillic and CJK text). `cargo test`
compares each pixel; after an intended change run
`cargo xtask test golden --bless`, **look at the PNGs you changed**, and
commit them. `target/orpheus/golden.html` shows old, new and the
difference side by side. The manual's screenshots are copies of goldens
(`cargo xtask site shots`), so they follow automatically.

## 6. The gate

Before a change is done:

```sh
cargo xtask gate --no-dev
```

It runs `cargo fmt --check`, clippy with warnings as errors (both
workspaces), `xtask test all`, the brand and site checks, the firmware
build, `size --check` and `check-resident`, and reports each step as
passed, FAILED or not run ([07 §6](../plan/07-tooling-testing.md)).
CI runs the host part on every push.

## 7. A first change

Say the About screen should show the card's file system.

1. Find the screen: About is an info list (`ListKind::Info` in
   `crates/orpheus-apps/src/screens/lists.rs`). Screens draw through the
   toolkit in `crates/orpheus-ui` and get their data from service traits
   in `crates/orpheus-apps/src/services.rs`, which the simulator
   (`sim/src/apps.rs`) and the firmware (`fw/firmware/src/services.rs`)
   implement.
2. Add the row, a service method if the data is new (implemented in both
   back ends), and a string in `strings.rs` (append; ids are never
   reused) with translations in `sim/src/packs.rs`.
3. `cargo run -p orpheus-sim -- --script "…shot:about"` and look at it.
4. `cargo xtask test golden --bless`, look at every changed PNG.
5. `cargo xtask gate --no-dev`, then commit with the work-package id in
   the subject (see [CONTRIBUTING.md](../../CONTRIBUTING.md)).

More recipes: [how-to](how-to/).

## 8. Working with the player safely

The player has a lithium cell and its owner's music; the full rules are in
[`CLAUDE.md`](../../CLAUDE.md) and [07 §7](../plan/07-tooling-testing.md).
In plain words:

* **Install the udev rule** once (`tools/udev/99-orpheus.rules`, see
  [install](../user/install.md)).
* **Never write the flash chip's first 4 KiB or its last 128 KiB.** The
  normal commands cannot; only commands with `--i-have-a-backup` touch
  more, and only after `xtask dev backup-verify` passed.
* **Battery and charger settings** use only the values the original
  firmware uses, and the charge current never goes above 200 mA.
* **Tests end in a reboot**, never with the player switched off or
  waiting for a key.
* **On the card, write only inside `/.orpheus/`.** Never format it.
* **One program talks to the player at a time.** If it stops answering,
  wait two minutes for the watchdog and boot counter, then ask; do not
  retry blindly.
* **Never connect the headphone output to a computer's input** while USB
  is plugged in: the output is direct-drive and the grounds fight
  ([01 §9](../plan/01-hardware.md)).

The device commands currently need the companion repository
([yoton-rockbox](https://github.com/timschmidt/yoton-rockbox)) for two
helper payloads and the backup: set `ORPHEUS_DIR` to it.
