# Orpheus — rules for workers

Pure-Rust, Embassy-based firmware for the Yoton YM03 (Actions ATJ2157:
Cortex-M4F, 320 KiB RAM, 8 MiB SPI NOR that is not memory-mapped). The plan is
in `docs/PLAN.md`; start every session there and in `docs/STATUS.md`.

## Orientation

* Plan documents: `docs/plan/01…09-*.md`. Work packages and their order:
  `docs/plan/08-milestones.md`. Do the package you were given; do not pull
  later work forward.
* Hardware facts: `docs/plan/01-hardware.md` first, then the datasheet text in
  `docs/vendor/`, then `docs/hw/*.md`. The Rockbox port in
  `/home/tim/Documents/GitHub/yoton/ext/rockbox/firmware/target/arm/atj2157/`
  is working reference code for the drivers it has. Treat
  `/home/tim/Documents/GitHub/yoton/` as read-only.
* One entry point for everything: `cargo xtask …` (`docs/plan/07-tooling-
  testing.md` §2).

## Hard rules (the player is real hardware with a lithium cell and the owner's music)

1. Never write NOR sector 0 or the vendor data at `0x7E0000` and above. The
   player runs the full layout since 2026-11-24 (decision D-2): Orpheus owns
   `0x001000–0x7DFFFF`, written only through `xtask dev stage` /
   `install-stage2` / `install-full` and the firmware-update path (UPD-1:
   `xtask dev update`, the on-player Update screen, the web installer's
   Update; inactive slot plus the old slot's magic word only, approved by
   the owner 2026-11-24). Only `xtask dev install-boot
   --i-have-a-backup` and `restore vendor --i-have-a-backup` may touch
   sector 0.
2. Before the first NOR write of a session, `xtask dev backup-verify`. If the
   backups in `/home/tim/Documents/GitHub/yoton/dumps/` are missing, stop.
3. PMU regulator and charger fields: only values the stock firmware uses
   (documented in `docs/plan/01-hardware.md` §7 and `docs/hw/system.md`).
   Charge current never above the 200 mA code. The eFuse block
   (`0xC0020000`) is read-only.
4. Never power the player off from an automated test; end every test in a
   reboot. Never leave it needing a key press. Exception (D-13, owner 2026-11-25):
   M13 discharge runs may run down to the firmware's own low-battery
   switch-off.
5. On the card, write only inside `/.orpheus/`. Never format, repartition or
   raw-write outside files the test created.
6. Do not send Bluetooth module commands 0x20–0x24, 0x28, 0x2C or 0x7F outside
   the package that owns them; never the upgrade commands without the user.
7. One process talks to the player at a time. If it stops responding, wait for
   the watchdog/boot-counter path (two minutes), then stop and ask the user;
   do not retry blind.
8. Ask the user before: switching to the full-flash layout, any step listed as
   a user decision (D-n) in `docs/PLAN.md` §5, anything that could need the
   case opened.

9. A second YM03 runs the factory firmware as the owner's reference
   player (07 §5.11). Never change its firmware or write to it other than
   adding test files under `ORPHEUS-REF/` on its card through its own card
   reader mode; never run `xtask dev` commands while it is on the
   workstation's USB, and never put it into recovery mode. If a `10d6`
   device appears that is not the Orpheus player, stop and ask.

## Engineering rules

* `no_std`, no `alloc`, no `core::fmt` in firmware crates; `defmt` for logs.
* Interrupts are masked with BASEPRI through the HAL's `critical-section`
  implementation. Never PRIMASK (`cortex_m::interrupt::free`, `cpsid i`): a
  page fault under PRIMASK is fatal.
* Code reachable from interrupt or fault handlers is resident;
  `xtask check-resident` proves it. Data paths with deadlines use DMA.
* Only `atj2157-pac`, `atj2157-hal`, `yoton-bsp`, `orpheus-pager`, `firmware`
  and the boot stages may know the hardware or use `unsafe`. Everything else
  is host-testable and tested on the host first.
* RAM and size budgets live in `budgets.toml`; exceeding one needs the
  measured numbers and the reason in the same commit.
* Every screen has a golden image; look at the PNG you produce.
* Every fixed bug becomes a regression test in the same commit as the fix
  (owner's policy, 2026-11-25): a host test named `regress_<bug>` whose doc
  comment names the bug, the date and the symptom, and which fails without
  the fix. Logic in hardware crates is factored into a host crate to make
  that possible; what cannot be (timing, the controller itself) gets an
  on-device check as well (`xtask dev suite` journey, `dev usb-cancel`-style
  tool or a soak replay). Fuzz and soak findings also keep their input in
  `tests/regress/`. `cargo test regress_` runs them all.
* Hardware facts learned go into `docs/plan/01-hardware.md` §9 and the source
  document in the same commit as the code that relies on them. Sequences
  copied from the vendor without understanding are marked `VERBATIM`.
* Do not copy code from the Rockbox tree (GPL) into Orpheus; licence decision
  D-1 in `docs/PLAN.md`.

## Done means

The gate in `docs/plan/07-tooling-testing.md` §6 passes (or each step that
could not run is reported as not run), `docs/STATUS.md` is updated with what
was done, measured numbers, open questions and new user checks, and the work
is committed with the package id in the subject line.
