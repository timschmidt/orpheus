# Architecture

One page on how Orpheus is put together. The depth is in
[02-architecture.md](../plan/02-architecture.md) (memory, boot, paging,
tasks, power, storage), [03](../plan/03-hal-drivers.md) (drivers),
[04](../plan/04-audio.md) (audio) and [05](../plan/05-ui-apps.md) (UI).

Three facts decide almost everything:

1. **320 KiB of RAM and no execute-in-place.** The 8 MiB flash is on SPI
   and not memory-mapped; all code must be copied to RAM before it runs,
   and the firmware is several times larger than the RAM.
2. **A Cortex-M4F at 24–288 MHz** (1.0 V core supply up to 144 MHz, 1.3 V
   at 288 MHz): decoding and synthesis take a noticeable share of real
   time, so scheduling and clock control are part of the audio design.
3. **No debugger.** Everything is observed over USB and through RAM that
   survives a reset.

## Memory

![Memory map](img/memory-map.svg)

SRAM0 holds the resident image, the statics, one shared **mode pool**
(re-partitioned for playback, pictures or USB storage) and the single
stack. SRAM1 is exactly the 128 × 128 RGB565 frame buffer, so a screenshot
is a memory read. SRAM2 is the code page cache and SRAM3 the cache for
constants and cold code (below), plus the PCM ring and the log ring. Budgets live in [`budgets.toml`](../../budgets.toml) and
`cargo xtask size --check` enforces them.

## Boot

![Boot chain](img/boot-chain.svg)

A slot that fails three starts in a row is skipped: stage 2 starts the
next one, after an update also the old, superseded image
(`orpheus_proto::boot`). The mask ROM's USB recovery mode (ADFU) is always
reachable: by a mailbox word, by holding Next, when every slot failed, and
when the boot record is invalid. That is what makes aggressive development safe. The slot
format (`"YFW1"`: header, resident image, 8 KiB pages) is in
[02 §2.3](../plan/02-architecture.md).

## Demand paging

![Demand paging](img/pager.svg)

`cargo xtask build` links the firmware twice: `xtask pagelink` decides
what is resident (everything reachable from interrupt and fault handlers,
listed by [`resident.toml`](../../resident.toml) and proven by
`cargo xtask check-resident`) and packs the rest into 8 KiB pages, hot
code first in clusters from [`hot.toml`](../../hot.toml). The pager itself
(`orpheus-pager`) has a host-side core with trace tests. A function that
must stay resident while the flash is busy needs `#[inline(never)]` as well
as a root: the proof cannot see inlined copies.

The code window (128 pages of 8 KiB) is full. Since PG-2 (2026-11-27)
**cold code**, which no deadline waits for (Books, the authenticator,
firmware updates' screens, the library build, rare settings screens, the
developer commands), runs from the **rodata window** in SRAM3, listed in
`resident.toml` `[bin.firmware.cold]`. `pagelink` places it where it shares
no physical cache line with a constant it reads; where it cannot, the pager
runs the one load instruction from a trampoline in resident RAM
(`orpheus_pager::bounce`). Per-byte loops (an update's data path, SHA-256)
must never be cold. Depth: [02 §3.6](../plan/02-architecture.md).

## Executors and tasks

![Priorities and executors](img/executors.svg)

Two Embassy executors share one stack. The RT executor runs at interrupt
level 3 and preempts thread mode, so a long UI operation (sorting a
directory, drawing a list) cannot starve the decoder. Critical sections
mask with BASEPRI, never PRIMASK: a page fault under PRIMASK would be
fatal. Tasks talk through `embassy-sync` channels and snapshot cells.
The card task on the RT executor decodes music, renders the MIDI
synthesiser in bounded pieces and runs the card chores (listings, library,
books, games saves) only when the PCM ring has room for them; the speech
voice runs in thread mode and is mixed after the DSP chain.

## Clock and power

`orpheus_power::governor` (host-tested) picks the clock level once a
second from the measured load: 24, 48, 96, 144, 192 or 288 MHz, the
supply raised before the clock goes up and lowered after it comes down.
**Power mode** sets its aim: *Automatic* runs the cheapest level that
carries the demand, never below the playing track's floor or a
peripheral's minimum (USB needs 48 MHz), climbing to 288 MHz by itself;
*Battery saver* pins the lowest level the peripherals in use allow;
*High performance* pins 288 MHz. Tests always run under Automatic.
Depth: [02 §5.4](../plan/02-architecture.md).

## Crates

![Crates](img/crates.svg)

* **`crates/`**: everything that does not need the hardware: UI toolkit
  and apps, player and library, file system, codecs, DSP, picture
  decoders, Bluetooth and USB protocol logic, settings store, power policy.
  All `no_std`, no `alloc`, tested on the host first.
* **`fw/`**: the second workspace, for `thumbv7em-none-eabihf`:
  `atj2157-pac` (registers), `atj2157-hal` (drivers and Embassy glue),
  `yoton-bsp` (this board), `firmware` (task wiring and the service
  implementations), the boot stage, payloads and bring-up images. Only
  these and `orpheus-pager` may use `unsafe`.
* **Desktop**: `sim/` runs the real apps over simulated services (golden
  images, key scripts); `xtask/` and `orpheus-host` build, test and talk
  to the player over USB.

The apps see the system only through the service traits in
`orpheus-apps` (`services.rs`); the simulator and the firmware each
implement them. That seam is what lets the same screens run on the
desktop, on the player and in the browser (`sim/web`, the website's Try
it page).

## Where to go next

* [Getting started](getting-started.md)
* [How-to guides](how-to/)
* [STATUS.md](../STATUS.md): what is built and measured
