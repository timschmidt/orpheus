# Contributing to Orpheus

Thank you for helping. Orpheus is firmware for a real player with a lithium
cell, so a few rules are stricter than usual; they are short.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting a bug

[Open an issue](https://github.com/timschmidt/orpheus/issues/new/choose)
and pick the bug template. Most useful:

* what you did, what you expected, what happened;
* the firmware build (Settings › System › About, or
  `cargo xtask dev info`);
* for a crash or restart, the output of `cargo xtask dev results`
  ([reading a fault record](docs/dev/how-to/read-fault-record.md));
* for a file that does not play or show, the file if you may share it.

Security problems go privately through [SECURITY.md](SECURITY.md).

## Proposing a change

1. **Start from the plan.** [docs/PLAN.md](docs/PLAN.md) and
   [08-milestones.md](docs/plan/08-milestones.md) list the work packages
   and their order; [docs/STATUS.md](docs/STATUS.md) says what is done. For
   anything larger than a fix, open an issue first and name the package.
2. **Set up**: [getting started](docs/dev/getting-started.md).
3. **Work on the host first.** Only `atj2157-pac`, `atj2157-hal`,
   `yoton-bsp`, `orpheus-pager`, `firmware` and the boot stages may know
   the hardware or use `unsafe`. Everything else is `no_std`, without
   `alloc` or `core::fmt` in firmware crates, and tested on the host.
4. **Every screen has a golden image.** Re-bless with
   `cargo xtask test golden --bless` and look at every PNG you changed.
5. **Budgets.** RAM and size budgets are in `budgets.toml`; exceeding one
   needs the measured numbers and the reason in the same commit.
6. **Run the gate**: `cargo xtask gate --no-dev` (CI runs the host part
   too). Steps that cannot run are reported as "not run", never as passed.
7. **Update STATUS.md** with what you did, measured numbers and open
   questions, and the user documentation (`docs/user/`) if users see the
   change.
8. **Commit** with the work-package id in the subject
   (`AU-5: gapless joins for FLAC`), and open a pull request.

## Working with the player

If you test on a YM03, read the safety rules in
[getting started §8](docs/dev/getting-started.md#8-working-with-the-player-safely)
and [CLAUDE.md](CLAUDE.md). The short version: back up the flash first,
never write its first 4 KiB or last 128 KiB, keep battery and charger
values to those of the original firmware, write on the card only inside
`/.orpheus/`, and end every test with a reboot.

## Code you did not write

Only port code under a permissive licence (MIT, Apache-2.0, BSD, zlib)
and keep its notice; list it in [THIRD-PARTY.md](THIRD-PARTY.md) if its
licence differs from Orpheus's. **Do not copy code from the Rockbox port
or any other GPL source**: it is used as a hardware reference only.
Material derived from the vendor's firmware or documents stays out of the
repository (decision D-7).

## Style

* `cargo fmt` (the settings are in `rustfmt.toml`) and clippy with
  warnings as errors.
* Documentation: plain, concise English, present tense, numbers with
  units; each fact lives in one place and other pages link to it. Facts
  learned about the hardware go into `docs/plan/01-hardware.md` §9 in the
  same commit as the code that relies on them.
* Interrupts are masked with BASEPRI, never PRIMASK.

## Licence

Orpheus is licensed under MIT OR Apache-2.0. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in the
work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
