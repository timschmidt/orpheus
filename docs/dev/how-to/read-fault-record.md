# Read a fault record

When the firmware crashes, its resident fault handler writes a **fault
record** to RAM that survives the reset (`0x137D00`, 256 bytes, format in
`crates/orpheus-proto/src/fault.rs`), shows the crash screen, and reboots
through the watchdog. The record stays until it is read and cleared.

## Read it

```sh
cargo xtask dev results
```

This switches the player to recovery mode (ymon) and prints the boot info
(which slot started, attempt count), the fault record, the log ring and
the result block of the last test. A fault line looks like this:

```
FAULT RECORD: UsageFault pc=0x00231004 lr=0x00104a2b sp=0x00137210 ipsr=4 cfsr=0x10000 [UNDEFINSTR] mmfar=0x0 bfar=0x0 hfsr=0x0
```

| Field | Means |
|---|---|
| kind | `HardFault`, `MemManage`, `BusFault`, `UsageFault`, or Orpheus's own: `StackGuard` (the MPU guard below the stack), `PageFaultInException` (a paged address touched where paging is not allowed), `Panic` (message in the text), `Watchdog`, `UnhandledIrq` |
| pc, lr | Where it happened and the return address |
| ipsr | 0 = thread mode (main executor); otherwise the exception number |
| cfsr | Fault status bits, decoded in brackets |
| mmfar, bfar | Faulting data address, when valid |

## Find the code

Addresses at `0x100000…` are resident; `0x200000…` are code pages and
`0x340000…` rodata pages (each page has its own address). Both are in the
paged ELF that `cargo xtask build` writes:

```sh
ADDR2LINE=$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-addr2line
$ADDR2LINE -Cfie target/orpheus/firmware.elf 0x00231004 0x00104a2b
```

Use the ELF of the build that crashed: the record carries the build id;
compare it with the slot header of `target/orpheus/firmware.yfw`.

## Common causes

* **UsageFault UNDEFINSTR at a page address** while the flash was being
  written: paged code ran while the NOR chip was busy. A NOR helper was
  inlined into a paged caller; mark it `#[inline(never)]` and make it a
  resident root.
* **StackGuard**: a deep call path; `metrics/static.csv` lists the
  largest frames.
* **PageFaultInException**: an interrupt handler reached paged code;
  `cargo xtask check-resident` should have caught it. Add the root to
  `resident.toml`.
* **Watchdog** with no other record: a task stopped making progress for
  about 3 seconds. The previous boot's last interrupt sample is kept at
  `0x137FDC` (exception) and `0x137FE0` (PC).

Write what you found into STATUS; a fault that a test can trigger becomes a
HIL test.
