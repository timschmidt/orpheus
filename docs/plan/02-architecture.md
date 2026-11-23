# 02 — Architecture

How the firmware is put together: memory, boot, demand paging, tasks, power,
storage, logging, and the crate layout. Hardware facts are in
[01-hardware.md](01-hardware.md); this document refers to them and does not
repeat register detail.

Three constraints decide almost everything here:

1. **320 KiB of RAM in four blocks, and no execute-in-place.** The SPI NOR is
   not memory-mapped. All code and constants that run must first be copied to
   RAM. A complete firmware is several times larger than the RAM.
2. **A 48 MHz Cortex-M4F today** (higher clock levels to be unlocked, 03 §2):
   an MP3 frame costs a noticeable share of real time, so scheduling and clock
   control are part of the audio design, not an afterthought.
3. **No debugger.** Everything is observed through USB and through RAM that
   survives a reset.

A fourth fact arrived late and is deliberately *not* a foundation: the
datasheet says this SoC can execute code directly from SPI NOR through a
hardware cache (01 §9 U-XIP-1). No register documentation exists and the
stock firmware does not use it. The software pager below is the baseline
because it is proven; the hardware cache is a time-boxed investigation that
would replace §3 if it succeeds.

The Rockbox port in `/home/tim/Documents/GitHub/yoton/ext/rockbox`
(`firmware/target/arm/atj2157/`) solved 1 with an MPU-driven demand pager and
proved it on this hardware (320 kbit/s MP3 with the UI running). yotonrs keeps
that mechanism and rebuilds everything above it around Embassy.

## 1. Memory map and budget

### 1.1 Physical memory (verified, FINDINGS §7)

| Block | CPU address | Size | Peripheral-DMA address | Notes |
|---|---|---|---|---|
| Mask ROM | `0x00000000` | 64 KiB | — | DMA engines see SRAM0 here instead |
| SRAM0 | `0x00100000–0x00137FFF` | 224 KiB | same | The only block the **USB** DMA is known to reach. ROM boot uses `0x100000–0x101B18`; boot record loads at `0x101000`; stage 2 at `0x102400` |
| SRAM1 | `0x00158000–0x0015FFFF` | 32 KiB | same | |
| SRAM2 | `0x00200000` + 32 KiB, **aliased every 32 KiB over 1 MiB** | 32 KiB | `0x10000000` | Off after a cold boot: clock gates, reset pulse and window registers must be set first (crt0 of the Rockbox port; 01 §"SRAM2/3 bring-up") |
| SRAM3 | `0x00300000` + 32 KiB, aliased likewise | 32 KiB | `0x18000000` | Same bring-up |

Unmapped addresses read as garbage and never fault, so a wild pointer is not
caught by the bus. The MPU is the only protection available.

### 1.2 Use of each block

```
SRAM0 224 KiB  0x100000 ┌──────────────────────────────┐
                        │ resident image               │  ≤ 72 KiB  vectors, pager, ISRs, executor,
                        │  (.text .rodata .data)       │            hot paths (see §3.4)
                        ├──────────────────────────────┤
                        │ .bss / .uninit               │  ≤ 20 KiB  task storage, channels, driver state
                        ├──────────────────────────────┤
                        │ caches                       │     8 KiB  FS sector cache, dir window, glyph cache
                        ├──────────────────────────────┤
                        │ mode pool                    │   104 KiB  re-partitioned per mode (§1.3)
                        ├──────────────────────────────┤
                        │ stack (MSP), guard below     │    12 KiB  one stack for thread mode, both
                        │                              │            executors and all interrupts
                        ├──────────────────────────────┤
               0x137C00 │ persistent area              │     1 KiB  fault record, result block, boot mailbox
               0x138000 └──────────────────────────────┘
SRAM1  32 KiB  0x158000   frame buffer 128×128 RGB565 (exactly 32 KiB)
SRAM2  32 KiB  0x200000   code page cache: 4 slots × 8 KiB (32 lines of 1 KiB)
SRAM3  32 KiB  0x300000   rodata page cache: 2 slots × 8 KiB        (0x300000–0x303FFF)
               0x304000   PCM output ring 12 KiB · log ring 2 KiB · pager tables
                          and other must-not-move statics 2 KiB     (0x304000–0x307FFF)
```

The numbers are the initial `budgets.toml` and are enforced by `xtask size
--check` (07 §4.2). They are targets to be corrected by measurement, never
silently exceeded. Slack at the start: about 7 KiB in SRAM0.

Notes:

* **One stack.** Embassy tasks are state machines stored in statics, so there
  are no per-thread stacks (the Rockbox port needed five). The single stack
  must hold the deepest UI call chain, plus a preempting decoder frame, plus
  nested interrupt frames. Big work buffers never go on the stack; decoders
  get their scratch from the arena. The stack is painted at boot and its
  high-water mark is part of every HIL report.
* **Stack guard.** MPU region 7 (highest precedence) is a 256-byte no-access
  region at the stack limit; hitting it is a fatal fault with a clear record.
  The pager therefore has six pool regions instead of the Rockbox port's seven
  (§3.3). If measurement shows this costs page faults, replace the guard with
  a canary checked by the watchdog task.
* **Frame buffer.** Kept full-size because the host reads it for screenshots
  and golden-image comparison, and because the vendor driver shows that the
  LCD controller can be fed by DMA from memory (03 §"Display"). When the panel
  is off, SRAM1 may be lent to a memory-hungry job (for example a 24-bit/96 kHz
  FLAC block buffer); this is an optimisation for later, with the rule that
  the borrower gives it back before the panel is switched on.
* **PCM ring in SRAM3.** The general DMA reaches SRAM3 through `0x18000000`
  (used for SD and NOR transfers by the Rockbox port). Confirm with the DAC
  channel in bring-up (01, U-DMA-1); if it fails the ring moves into the mode
  pool.
* **USB buffers** must be in SRAM0 until U-USB-6 shows the USB DMA can reach
  other blocks.

### 1.3 The mode pool

The largest RAM consumers never run at the same time, so one 104 KiB region is
handed out by a small **lease allocator** (`yoton-audio::pool`): a lease is a
`&'static mut [u8]` slice with an owner tag; the pool has a fixed partition
table per mode, and a mode switch waits until every lease of the old mode has
been returned.

| Mode | Partition of the 104 KiB |
|---|---|
| Playback | stream ring 32 KiB · codec arena 72 KiB (each decoder declares its need at open; the remainder is lendable to background jobs: library scan, thumbnails, directory sort) |
| Recording | encoder arena 24 KiB · write-behind buffer 80 KiB (covers a 400 ms card stall at 48 kHz/16-bit stereo) |
| USB storage / disk emulation | two 48 KiB transfer buffers (read-ahead and send in parallel) · 8 KiB extent table and SCSI state |
| Pictures (no audio playing) | decoder arena 104 KiB; while audio plays only the codec arena's lendable remainder |
| Idle | everything lendable |

Background borrowers must return memory when asked (their jobs are restartable
by design); a decoder that needs more than is free makes the pool call the
borrowers back before playback of that file starts.

### 1.4 What must never be forgotten about RAM here

* No heap. No `alloc`. The firmware binary has no global allocator, so a
  dependency that needs one does not link.
* Every `static` is a line in the size report. Large `static mut` buffers
  outside the pool need a budget entry.
* An Embassy task's future is a static whose size is the largest set of locals
  alive across an `.await`. Never hold a buffer across an await in a task
  body; borrow from the pool or a `StaticCell` instead. `xtask size` prints
  the ten largest task futures.

## 2. Flash layout and boot chain

### 2.1 What is on the NOR today

8 MiB Zbit NOR (JEDEC `5E 40 17`) on SPI0. The vendor image is stored
**scrambled**; the SPI controller descrambles on the fly when its CTL bit 12 is
set. Everything yotonrs writes is stored plain.

| NOR range | Content today |
|---|---|
| `0x000000–0x0001FF` | Boot stage 1 of the Rockbox port (512-byte type `0xA5` boot record; replaces the vendor record) |
| `0x000200–0x000FFF` | Vendor second-stage bytes (untouched) |
| `0x001000–0x760BFF` | Vendor firmware (LFI image at `0xF400`, directories at `0xF000`/`0x10000`, 96 files) |
| `0x762000–0x7CBFFF` | Rockbox page store (`"YMPG"`) |
| `0x7CC000–0x7DEFFF` | Rockbox resident image (`"YRES"`) |
| `0x7DF000–0x7DF3FF` | Copy of the vendor boot record (descrambled) |
| `0x7DF400–0x7DF7FF` | Boot stage 2 of the Rockbox port (`"YST2"`) |
| `0x7E0000–0x7FFFFF` | Not free (vendor data; 03 §"NOR" asks the worker to identify it — probably the stock settings area) |

### 2.2 Boot chain

```
mask ROM ──► stage 1 (NOR 0, 512 B, loaded to 0x101000)
               │  reads 1 KiB from NOR 0x7DF400 to 0x102400, checks "YST2", jumps to +0x11
               ▼
             stage 2 (≤ 1 KiB)                     decides what to start
               │  boot mailbox "ADFU"  ──────────► ROM USB recovery (ADFU)
               │  Next key held        ──────────► ROM USB recovery
               │  OK key held          ──────────► vendor firmware (dual-boot layout)
               │  boot counter ≥ limit ──────────► other slot (full layout) → vendor → ADFU
               │  valid firmware slot  ──────────► copy resident image to 0x100000, jump
               ▼
             resident image: crt0 → SRAM2/3 on → clocks → pager → Embassy → tasks
```

* **Stage 1 stays as installed.** Its contract (load 1 KiB from `0x7DF400`,
  magic `"YST2"`, entry at offset `0x11`) is all yotonrs needs, so sector 0 is
  not rewritten during development. A Rust rewrite of stage 1 (for a tree with
  no C at all) is the last, optional step of the release milestone because it
  is the only work that needs a sector-0 write.
* **Stage 2 is rewritten in Rust** (`boot/stage2`, `#![no_std]`, `#![no_main]`,
  `opt-level = "z"`, no panics, no formatting, global-asm entry). It must fit
  1 KiB including the trampoline that survives being overwritten by the image
  it loads (see `payloads/ym03stub.c` for the technique: the copy loop runs
  from the top of SRAM0). It keeps the existing mailbox words (`0x137FF4`
  "ADFU", `0x137FF8` attempt counter) and adds a test-mode word (07 §5.2).
  Key thresholds: `button-yotonym03.c`. It must tolerate ADCs that are not
  running (reads of 0 select nothing). Its temporary stack and trampoline
  must lie outside the persistent area `0x137C00–0x137FFF` (the C stub puts
  its stack at `0x137F00`, which would destroy the result block and fault
  record that the test harness reads after a reboot).
* The **ROM recovery mode is always reachable**: by mailbox, by key, after
  three failed starts, and by the ROM itself when the boot record is invalid.
  That is what makes aggressive development safe.

### 2.3 Firmware slot format (`"YFW1"`)

```
+0x0000  header, one 4 KiB sector
           magic "YFW1", format version, build id (16 B), layout id
           resident: offset, length, load address (0x100000), entry, sum32
           pages:    offset, code page count, rodata page count, crc32
           resources: directory offset/length (fonts, icons, language packs, external references)
           verified: 0xFFFFFFFF until the firmware has checked the page CRC once, then programmed to 0
           header crc32
+0x1000  resident image (flat binary)
+…       page store: code pages, then rodata pages, 8 KiB each, sector aligned
+…       resources
```

Stage 2 checks the header magic, bounds and the resident `sum32` (cheap enough
for 1 KiB of code). The firmware checks the page-store CRC on the first boot
of a new image, before enabling anything that could damage data, and then
clears `verified` (NOR can clear bits without an erase).

### 2.4 Layouts

**Dual-boot layout (default; keeps the stock firmware bootable):**

| NOR range | Use |
|---|---|
| `0x000000–0x760BFF` | untouched |
| `0x762000–0x7D9FFF` | slot A, 480 KiB |
| `0x7DA000–0x7DDFFF` | settings store, 4 sectors |
| `0x7DE000–0x7DEFFF` | authenticator vault, 1 sector (mirrored inside the settings store's wear-levelling if it grows) |
| `0x7DF000–0x7DFFFF` | vendor boot record copy, stage 2 (as today) |

480 KiB will not hold every feature. Cargo features select what a dual-boot
build contains; `xtask size --check --layout dual` reports the margin. In this
layout the 16 px fallback font is **read in place from the vendor image**
(`UNICODE.FON`, 2 162 688 bytes = 65 536 glyph records of 33 bytes: a 16 × 16
bitmap, two bytes per row, most significant bit left, followed by one advance-
width byte; verify in WP-UI-1). `xtask` finds the file's NOR offset from the
backup dump and records it in the slot's resource directory as an external,
scrambled resource; the NOR driver reads it with the hardware descrambler on.

**Full layout (opt-in, replaces the stock firmware; all 8 MiB):**

| NOR range | Use |
|---|---|
| `0x000000–0x000FFF` | stage 1 |
| `0x001000–0x1FFFFF` | slot A (≈ 2 MiB) |
| `0x200000–0x3FFFFF` | slot B (2 MiB; the updater writes the inactive slot) |
| `0x400000–0x63FFFF` | resources: font pack generated from an open font (GNU Unifont, same 33-byte record format), language packs, wallpapers |
| `0x640000–0x7CFFFF` | reserved |
| `0x7D0000–0x7DEFFF` | settings, vault, step history |
| `0x7DF000–0x7DFFFF` | stage 2 |
| `0x7E0000–0x7FFFFF` | spare |

Switching to the full layout is a deliberate, user-approved step
(`xtask dev install-full --i-have-a-backup`). Returning to stock is
`xtask dev restore vendor` (whole-chip restore from `dumps/nor_backup_8m.bin`
through ADFU).

### 2.5 Updating

* **From the host** (always available): `xtask dev stage` through ADFU.
* **From the player** (Settings → System → Update firmware, `.yfw` file on the
  card): the running firmware verifies the file, resolves its extents on the
  card, then starts a **RAM-only installer** (a self-contained ≤ 64 KiB image
  inside the `.yfw`, no paging) passing it the extent list. The installer
  needs only the SD block driver and the NOR driver, shows a progress bar,
  programs and verifies the target slot (the inactive one in the full layout,
  slot A in the dual layout), and reboots. A power cut leaves either the old
  slot (full layout), or an invalid slot A that stage 2 skips in favour of the
  stock firmware, with ADFU as the last resort.

## 3. Demand paging

### 3.1 Mechanism (as proven by `pager-atj2157.c`)

SRAM2 and SRAM3 ignore address bits above bit 14, so the same 32 KiB of RAM
appears at 32 addresses in each 1 MiB window. The linker gives every 8 KiB
page of non-resident code its own virtual address:

```
code page p   : VA = 0x200000 + (p >> 2) * 0x8000 + (p & 3) * 0x2000     slot = p & 3   (SRAM2)
rodata page q : VA = 0x340000 + (q >> 1) * 0x8000 + (q & 1) * 0x2000     slot = q & 1   (SRAM3, low half)
```

Up to 128 code pages (1 MiB) and 48 rodata pages (384 KiB) can exist.

MPU region 0 denies `0x200000–0x3FFFFF` except its sub-region
`0x300000–0x33FFFF`, where SRAM3 is plain RAM. Pool regions each map one page
(8 KiB, eight 1 KiB sub-regions) with only the **lines** actually loaded
enabled. Touching anything else raises MemManage. The handler:

1. works out page, slot and line from the fault address (or the stacked PC for
   instruction faults);
2. finds or allocates a pool region for the page (least recently faulted, never
   the page containing the faulting PC);
3. if the physical line already holds this page's data (it survives region
   reuse), only re-maps; otherwise takes the line from whichever page has it,
   reads 1 KiB from NOR with the MPU briefly disabled (FAST READ `0x0B`, SPI
   RX drained by DMA channel 0, mode `0x00200088`, SPI clock 48 MHz: about
   180 µs), updates the tables, re-enables the MPU;
4. returns, and the faulting instruction runs again.

Instruction fetches, data reads, calls, returns and function pointers all
work without the compiler knowing anything about it. An access that straddles
two lines reports its first address; the handler handles the second line on
the repeat fault (see the "straddling" branch in the C code — port that logic
exactly, with host tests replaying recorded traces, 07 §3.1).

### 3.2 Rules that paging imposes on all firmware code

1. **Faults are taken at priority 0 and everything else is masked with
   BASEPRI, never PRIMASK.** With PRIMASK set a page fault escalates to
   HardFault and the system dies. Therefore:
   * `critical-section` gets a **custom implementation** in `atj2157-hal` that
     raises BASEPRI to the highest ordinary interrupt level. The `cortex-m`
     feature `critical-section-single-core` must stay disabled; a build check
     greps the final ELF for `cpsid i` outside the boot stages.
   * `cortex_m::interrupt::free`, `interrupt::disable` and friends are banned
     (clippy `disallowed-methods`).
2. **The pager, the NOR driver, the fault handlers and the crash screen are
   resident and reach nothing paged.** Proven by `xtask check-resident`
   (07 §4.3).
3. **Interrupt handlers should be resident** for latency (a fault inside a
   handler works but stalls lower priorities for the read). The same tool
   enforces it for the handlers that have deadlines (audio, time driver).
4. **A fault holds off all interrupts for the duration of a line read.** Every
   data path with a deadline shorter than about 1 ms must therefore be carried
   by DMA: DAC, ADC, SD, NOR, USB, the Bluetooth UART. No byte-at-a-time
   interrupt service anywhere in the data plane.
5. **Nobody but the pager talks to the NOR chip without the pager knowing.**
   Program and erase operations make the chip unavailable for milliseconds to
   hundreds of milliseconds. The resident NOR driver keeps a `busy` state; a
   fault that arrives while the chip is busy issues *program/erase suspend*
   (`0x75`), waits for the suspend latency, reads the line, then *resumes*
   (`0x7A`). Suspend/resume support of this chip is to be confirmed from its
   SFDP table and by test (01, U-NOR-2). If it is not available: settings
   writes are deferred to moments when audio is stopped, and the firmware
   pre-faults the code it needs and blocks the RT executor for the duration.
6. **Watch the working set.** Four code slots hold at most 32 lines. A loop
   that alternates between more hot lines than fit thrashes at 180 µs per
   miss. Hot loops are either resident (§3.4) or packed together by profile
   (§3.5).

### 3.3 Implementation plan (`yoton-pager`, with `xtask pagelink`)

* `yoton-pager` (resident, `no_std`, `unsafe` allowed): MPU setup, the line
  cache and region pool as a pure data structure (`PagerCore`, host-testable),
  the MemManage entry in `global_asm!`, the NOR line reader (shared with the
  NOR driver), statistics, optional fault log ring, the stack guard region.
  Six pool regions (1–6), region 0 deny, region 7 stack guard.
* Sections larger than a page: pages `4k … 4k+3` of one alias block are
  contiguous in virtual address space, so a code section may cross page
  boundaries **inside a 32 KiB group** but never across groups. Rodata groups
  are 16 KiB (two pages). Anything larger (fonts, big tables) is not linked as
  rodata at all: it is a **resource** read through the NOR driver's API with a
  small cache (§7).
* `xtask pagelink` (model: `tools/ym03_pagelink.py`):
  1. link once with a linker script that puts everything in a huge fake
     resident region and emit a map; Rust already gives every function and
     static its own input section;
  2. classify each input section: resident (by rule, below), paged code,
     paged rodata; RAM sections are untouched;
  3. order paged sections using `hot.toml` (profile clusters first, in order;
     then by crate and call-graph proximity), fill 32 KiB code groups and
     16 KiB rodata groups;
  4. generate the final linker script with one output section per group at
     its alias address, marked non-loaded; link again;
  5. extract the resident image and the page store from the ELF, write the
     slot image and `layout.json` (section → page, sizes, margins).
* Classification rules live in `resident.toml`: path-prefix patterns over
  demangled symbol names (`yoton_pager::`, `atj2157_hal::irq::`,
  `embassy_executor::raw::`, `embassy_time_driver`, `memcpy`/`memset`
  builtins, …), explicit sections (`.text.resident.*`, `.rodata.resident.*`
  via a `#[resident]` attribute macro for our own code), and whatever
  `check-resident` proves reachable from the vectors.
* Fat LTO merges crates into one object, so rules work on symbol names, not
  object file names.
* Development order: (a) `PagerCore` and host tests, (b) pagelink on a toy
  firmware in the simulator of the MPU, (c) on the device with the NOR page
  store written by `xtask dev stage` and the resident image loaded to RAM, (d)
  from NOR via stage 2.

### 3.4 What is resident (target ≤ 72 KiB)

Vectors and crt0; the pager and NOR driver; fault handlers and the crash
screen (own 5 × 7 hex/ASCII font, draws straight to the LCD); the BASEPRI
critical section; the Embassy executor core, wakers and the time driver; DMA
and peripheral interrupt handlers; the PCM ring and DMA feeder; the key
scanner's sampling step; `memcpy`/`memset`/`memmove`/`memcmp`; the watchdog
feed. Then, **by measurement only**: per-frame inner loops of the codec in
use cannot all be resident (there are many codecs), so codecs are paged and
packed (§3.5); the SD block read path and the FAT cluster-chain step used by
the stream reader are resident candidates if profiles show them thrashing.

### 3.5 Profile-guided packing

`hot.toml` lists clusters: ordered section-name patterns that must be packed
adjacently (same group, same lines where possible): `mp3-frame`, `flac-frame`,
`vorbis-frame`, `opus-frame`, `browse-scroll`, `dir-scan`, `now-playing-tick`,
`sd-read`, `usb-msc-pump`, … A cluster that fits the cache (≤ 24 lines, leaving
8 for everything else) runs fault-free after warm-up. The fault profiler
(07 §5.6) regenerates the file; the HIL `pager` suite reports faults per
second for each scenario and the gate has thresholds (steady-state playback
with the screen off: < 5 faults/s).

## 4. Tasks, priorities and time

### 4.1 Priority levels

The NVIC has exactly four priority levels (datasheet; 01 U-CPU-1), which is
what the design needs:

| Level | What runs there | Rule |
|---|---|---|
| 0 | MemManage (pager), BusFault, UsageFault; HardFault above | Resident, no blocking beyond a NOR line read |
| 1 | Deadline interrupts: time-driver timer, audio DMA events, UART/DMA events of the Bluetooth link | Resident; only move pointers and wake tasks |
| 2 | Other peripheral interrupts: USB, SD/DMA completion, I2C, RTC, PMU, GPIO | Short; wake tasks |
| 3 | **RT executor** (`embassy_executor::InterruptExecutor` on an unconnected NVIC line pended in software, U-IRQ-2) | Audio pipeline and other soft-real-time tasks |
| thread | **Main executor** (`embassy_executor::Executor`, WFI when idle) | UI, apps, services, background jobs |

BASEPRI critical sections mask levels 1–3.

Because the RT executor preempts thread mode, a long UI operation (sorting a
directory, decoding a JPEG, drawing a list) cannot starve the decoder. This
replaces the Rockbox port's scattered UI yield points.

### 4.2 Tasks

| Task | Executor | Owns |
|---|---|---|
| `audio::reader` | RT | Stream ring fill from the card (multi-block DMA straight into the ring) |
| `audio::decode` | RT | Decoder, DSP chain, PCM ring fill; during recording: encoder and writer |
| `audio::sink` | RT | DAC/ADC DMA bookkeeping, underrun accounting, hardware volume ramps |
| `bt::stream` | RT | Bluetooth audio packet pump (if a role needs it) |
| `usb::pump` | RT | Mass-storage/disk-emulation data pump (card ↔ USB), only in those personalities |
| `ui` | main | Screen stack, drawing, display flush requests |
| `input` | main | Key sampling (10 ms; 50 ms when the panel is off), debounce, repeat/hold, lock |
| `playback` | main | Queue, transport state machine, resume, bookmarks; commands the audio tasks |
| `storage` | main | Card detect, mount/unmount, hot-plug, volume mutex owner for metadata work |
| `library` | main | Scanner and index builder (lowest urgency; yields to audio pressure) |
| `power` | main | PMU, battery gauge, charger state, clock governor, idle/auto-off timers, shutdown |
| `clock` | main | RTC, alarms, timers, stopwatch timestamps |
| `settings` | main | Key-value store, debounced writes |
| `radio`, `recorder`, `sensors`, `bt::control`, `usb::control` | main | One per subsystem |
| `watchdog` | main | Feeds only if both executors reported progress in the last period |
| `devchan` | main | Developer channel: key injection, statistics, test mailbox |

Communication: commands as small `enum`s through `embassy_sync` channels;
state as snapshot cells (`Watch`-style) that the UI reads. Mutex flavour by
need: `NoopRawMutex` inside one executor, the BASEPRI critical-section mutex
across executors or with interrupts. Decoders are synchronous code called
from `audio::decode`; one call decodes at most one frame, then the task
yields.

### 4.3 The audio deadline, concretely

The PCM ring holds 12 KiB ≈ 68 ms at 44.1 kHz stereo. `audio::decode` is woken
when the ring falls below two thirds; it must then be scheduled within
about 40 ms, which holds because only interrupts and page faults (bounded,
§3.2) are above it. A decoder whose frame is larger than the free ring space
(FLAC block of 4096 samples = 16 KiB) keeps the decoded block in its arena and
hands it over in pieces. Card latency is absorbed by the stream ring (32 KiB:
0.8 s of 320 kbit/s MP3, 0.3 s of typical FLAC), refilled when below half in
bursts of whole clusters so the card can idle between bursts.

### 4.4 Time base

`embassy-time` with a driver in `atj2157-hal::time` built on the RTC-block
timers, which run from the 24 MHz crystal and so do **not change with the core
clock** (SysTick does, and is not used). Timer 0 free-runs as the time base
(extended to 64 bits in software); a second timer provides the alarm for the
Embassy timer queue; details and the fallback if only one usable timer exists
are in 03 §"Timers". Tick rate 1 MHz. In the deepest sleep state the 24 MHz
oscillator stops; time is re-synchronised from the RTC on wake.

### 4.5 Idle

The main executor sleeps with WFI when no task is ready (U-CPU-2: confirm that
WFI stops the core clock and that every wake source used reaches the NVIC).
The clock governor (§5) lowers the core clock when the load allows, and the
`power` task enters deeper states when the whole system is idle.

## 5. Power and clock management

Hardware detail (PLLs, dividers, regulator fields, charger, standby states)
comes from [docs/hw/system.md](../hw/system.md) and 03 §"Clocks", §"PMU". The
software structure is fixed here.

### 5.1 Clock governor

* A small table of **performance levels** (03 §2.1): `Idle` 6 MHz, `Low`
  24 MHz (crystal, PLL off), `Base` 48 MHz, `Mid` 96 MHz, `High` 144 MHz — all
  at a 1.0 V core supply — and `Turbo` 192 MHz at 1.1 V. Each is a row of the
  vendor's own table, admitted only after the level self-test. Lowering the
  supply from the 1.2 V reset default to 1.0 V is the largest single power
  saving available and happens at start-up.
* Tasks hold RAII **requests** (`PerfRequest::new(Level)`); the governor runs
  the highest requested level. Typical requests: UI interaction → `Base` for
  2 s after the last key; decoder → the lowest level at which measured decode
  time per frame is below 50 % of real time for the current stream (learned
  per codec/bitrate, then cached); USB storage → `High`; picture decode,
  library scan → `High` while running; idle → `Low`.
* A level change is a single resident routine that runs with interrupts
  masked: raise voltage first when going up, detour through the crystal while
  the PLL retunes, lower voltage last when going down. Peripheral clocks do
  not follow the core clock, except that the NOR and LCD controllers need care
  (03 §2.3): the pager's SPI clock must come from a source the governor does
  not retune, and every level is validated by a page-store CRC pass before it
  is allowed in the table.
* Audio clocks come from the audio PLL and are independent of the core level.

### 5.2 Peripheral power

Every driver owns its clock gate and reset bit and turns them off when its
last user goes away: LCD controller and backlight PWM with the panel, SD clock
between bursts, ADC when not recording or scanning keys, DAC and headphone
amplifier (with the pop-free ramp) a few seconds after audio stops, tuner in
power-down when the radio is off, Bluetooth module off when disabled, USB
controller and PHY off without VBUS, I2C between transactions. The `power`
task exposes the gate map to the Diagnostics screen and the HIL `power` suite.

### 5.3 System states

| State | Entered | What stays on | Leaves by |
|---|---|---|---|
| **Active** | Key press, USB host | Everything needed | — |
| **Screen-off play** | Backlight/lock timeout while audio runs | Core at the decoder's level, audio path, card in bursts, key scan at 20 Hz | Key, end of queue |
| **Idle** | Nothing playing, screen off | Core at `Low`, WFI, key scan; sensor task if step counting is on | Key, alarm, USB, motion |
| **Deep idle** | Idle for a minute | Our own state (the vendor has none): PLLs off, core at 6 MHz or on the 32 kHz clock, every idle clock gated, WFI; timers wake it to poll keys and the sensor | Key, alarm, USB, motion |
| **Standby (S3)** | Auto-off time reached while alarms or always-on step counting are wanted; also the candidate for "instant on" | The PMU's documented S3 state: core supply lowered (0.9 V default), wake by key, DC5V, RTC alarm, WIO1 pin. **Unused by stock; whether RAM survives and how it resumes is U-PMU-2.** If it does not pan out, deep idle takes its place | Wake source; resume in place, or restart with the RAM image intact |
| **Off (S4)** | Power menu, auto-off with nothing pending, battery empty | PMU off; RTC running | Power key long press, DC5V insert, RTC alarm (wake-enable bits, 01 §7) |

Auto-off, sleep timer and low-battery handling all go through one shutdown
path that saves the resume record and settings and unmounts the card.

### 5.4 Power modes offered to the user

* **Automatic** (default): governor as above.
* **Battery saver**: animations off, backlight capped, UI level capped at
  `Base`, decoders at their minimum level, spectrum/meters off, Bluetooth
  transmit power reduced where controllable, library scan only on USB power.
* **High performance**: minimum level `High` while the screen is on, largest
  read-ahead, used for the USB storage and disk-emulation personalities
  automatically.

### 5.5 Battery and charging

The battery ADC gives a voltage; the gauge is a discharge-curve table with
load compensation (different tables for screen on/off and charging), filtered
so the percentage never jumps or rises on battery. The charger is controlled
by the PMU: **use the vendor's configuration values unchanged** (07 §7 rule
4). Charging state, charger type (host vs wall adapter, by SOF detection as
the stock kernel does) and "full" are reported to the UI.

## 6. Storage stack

```
apps / audio / usb classes
        │  files, directories, extents
yoton-fs:  volume manager ─ FAT12/16/32 ─ exFAT ─ long names ─ path walk
        │  512-byte sectors; metadata through an 8-sector cache, file data by direct multi-block transfer
sd protocol layer: init, CMD6 high-speed switch, CMD17/18/24/25 (+CMD23), CMD12, CMD13 busy polling, error recovery
        │
atj2157-hal::sd: SD0 host controller + DMA channel 4, async completion, clock/gate control
```

Requirements:

* Async, no allocation, bounded stack. File data reads and writes go by DMA
  directly between the card and the caller's buffer (stream ring, USB buffer,
  write-behind buffer); only metadata passes through the sector cache.
* **Open files carry an extent cache** (a few `(first cluster, run length)`
  entries, refilled while reading), so steady-state streaming never walks the
  FAT. The same extent API feeds the firmware updater and USB disk emulation.
* **Directories are iterated, never loaded.** Sorting is the caller's business
  (05 §7.2, 04 §"Directory index").
* The volume is guarded by an async mutex with short holds: one directory
  step, one cluster allocation, one data transfer of at most 64 sectors. The
  audio reader therefore waits at most one such operation for the UI.
* Write safety: FAT updates in an order that can only leak clusters, never
  cross-link (allocate → write data → update directory entry); exFAT
  allocation bitmap likewise; the dirty flag / volume flags handled per spec;
  everything flushed before USB storage mode hands the card to the host and
  before power-off.
* FAT32 is what the owner's 256 GB card uses today; exFAT is what cards come
  formatted with. Both are required. Whether to adopt an existing crate or
  write `yoton-fs` is decided in WP-FS-1 from
  [docs/research/rust-ecosystem.md](../research/rust-ecosystem.md) §5
  (candidates found there: `embedded-fatfs` for FAT and `exfat-slim` for
  exFAT, both async over `block-device-driver`; the Embassy project's `sdio`
  crate for card initialisation) with these criteria: async without allocation, exFAT, extent access, bounded RAM
  (≤ 8 KiB for the cache plus ≤ 256 B per open file), write support. If no
  crate meets all of them, write it; the file-system test plan (07 §3.1) is
  the specification.
* **Settings store**: a wear-levelled key-value log in the NOR settings
  sectors: `sequential-storage` (research §12) over the suspend-aware NOR
  driver's `embedded-storage-async` interface, or a 300-line own log if it
  does not fit the RAM or code budget. Values are small
  fixed structs with a version byte. Writes are debounced (2 s) and coalesced.

## 7. Resources and fonts

A **resource** is read-only data that is too big or too rarely used to be
linked: font packs, language packs, wallpapers, the installer image. The
slot's resource directory maps an id to `(NOR offset, length, flags)`;
`ResourceReader` reads ranges through the resident NOR driver (synchronously —
a 1 KiB read is under 200 µs — so text drawing stays synchronous).

Fonts:

* **UI font** (12 px, proportional; Latin, Latin Extended-A, Greek, Cyrillic,
  symbols) and **large numerals** are converted at build time from open-
  licensed sources (BDF bitmap fonts for the UI face; a TrueType face
  rasterised by `xtask` for the numerals) into a compact format: range table,
  per-glyph advance and bounding box, 1-bit rows. They are ordinary paged
  rodata, split so no object exceeds the rodata group size.
* **Fallback font** (16 px, whole Basic Multilingual Plane): a resource in the
  33-byte direct-indexed format of §2.4. A 64-entry glyph cache in RAM
  (2.2 KiB) makes repeated text cheap.
* WP-UI-1 picks the faces; candidates and licences are listed there. Judge
  legibility from simulator screenshots at 1× scale, not from the font's
  reputation.

## 8. Logging, faults, watchdog

* **Logging** uses `defmt` with a global logger that writes frames into the
  persistent log ring (2 KiB in SRAM3): the `defmt-persist` crate if it fits
  (research §11), else 60 lines of our own. No RTT. `xtask dev log` reads the
  ring through the developer channel or, after a crash, through ADFU, and
  decodes it with the ELF. Format strings never reach the device, which keeps
  both flash and resident RAM small. `core::fmt` is **not** used in firmware
  crates (`xtask size` fails the gate if `core::fmt::write` is linked);
  user-visible numbers are formatted by a small own module.
* **Fault record**: any fatal fault (HardFault, BusFault, UsageFault,
  unserviceable MemManage, stack guard, panic) stores kind, PC, LR, status
  registers, fault address, active exception number and build id in the
  persistent area, draws the resident crash screen (fault kind and PC in hex,
  "restarting"), waits 3 s or a key, and reboots through the watchdog (the
  SoC ignores `SYSRESETREQ`).
* **Boot counter**: stage 2 increments it; the firmware clears it after 30 s
  of healthy running. Three failures in a row end in ADFU (dual layout: after
  trying the stock firmware is *not* automatic; ADFU keeps the evidence).
* **Watchdog**: armed by the ROM; register and period table in 03. Fed by the
  `watchdog` task only when the RT executor, the main executor and the UI
  loop have all made progress. The pager feeds it during a fault (forward
  progress). Long legitimate stalls (firmware install) feed it explicitly.
* **Events**: a 32-entry ring of coded events (boot reason, mount errors,
  underruns, faults survived) kept in the settings store for the Diagnostics
  screen.

## 9. Crate layout

```
yotonrs/
├── Cargo.toml                 workspace; profiles; lints
├── rust-toolchain.toml        pinned stable + thumbv7em-none-eabihf
├── .cargo/config.toml         target, rustflags, xtask alias
├── budgets.toml  resident.toml  hot.toml
├── crates/
│   ├── atj2157-pac            registers (hand-written, one module per block)
│   ├── atj2157-hal            drivers + Embassy glue: clocks, gpio/mfp, dma, nor, sd, lcm, pwm, dac, adc,
│   │                          usb (embassy-usb-driver), uart, i2c, rtc, pmu, timers/time driver, wdt, critical section
│   ├── yoton-pager            §3
│   ├── yoton-bsp              this board: pins, key ladders, panel init, tuner, motion sensor, Bluetooth module, power policy
│   ├── yoton-proto            shared constants and wire formats (firmware ↔ boot ↔ host)
│   ├── yoton-fs               §6
│   ├── yoton-audio            pipeline, rings, pool, DSP, metadata, library index, playlists
│   ├── codecs/                yoton-codec-{wav,mp3,flac,vorbis,opus,speex,aac,…}  one crate each, host-testable
│   ├── yoton-img              JPEG/PNG/BMP/GIF/QOI streaming decoders and scaler
│   ├── yoton-ui               toolkit (05 §5)
│   ├── yoton-apps             screens and app logic over service traits
│   ├── yoton-usb              personalities and classes: MSC/SCSI disk + CD-ROM, HID, developer channel, (UAC)
│   ├── yoton-bt               Bluetooth: per docs/hw/bluetooth.md
│   └── yoton-otp              HOTP/TOTP and the vault
├── firmware/                  the binary: memory layout, task wiring, service implementations
├── boot/stage2/  boot/stage1/ boot stages
├── payloads/                  RAM payloads for ADFU (ymon, NOR programmer) — Rust
├── bringup/                   small RAM-only test images, one per driver
├── sim/                       desktop simulator and golden-image tests
├── xtask/  crates/yoton-host  host tooling
├── tests/                     corpus generators, HIL suites, regress inputs
└── docs/
```

Dependency rules: `yoton-ui`, `yoton-apps`, `yoton-audio`, codecs, `yoton-fs`,
`yoton-img`, `yoton-otp` never depend on `atj2157-*` or `yoton-bsp`.
`atj2157-hal` knows the SoC, not the board. `yoton-bsp` knows the board.
`firmware` is the only crate that knows everything.

### 9.1 Build profile

```toml
[profile.release]
opt-level = "s"        # codec and DSP crates override to 3 where the on-device benchmark justifies it
lto = "fat"
codegen-units = 1
panic = "abort"
debug = 2              # symbols stay in the ELF for defmt, profiling and check-resident; not loaded
overflow-checks = false   # arithmetic in codecs is explicitly wrapping/saturating; tests run with checks on
```

`rustflags`: `-C target-cpu=cortex-m4`, linker script arguments from
`pagelink`. Stable Rust only for firmware; nightly is allowed for host-side
fuzzing.

### 9.2 Third-party crates

Chosen in the work packages from
[docs/research/rust-ecosystem.md](../research/rust-ecosystem.md) (read its
"ten findings" first: Embassy's released crates and its `main` branch have
diverged — the USB driver traits changed and the mass-storage class exists
only on `main`, so the workspace pins one git revision of Embassy for all its
crates; executor features were renamed, e.g. `platform-cortex-m`; many of the
allocation-free codec crates it lists are months old and little used, so each
is adopted only after passing our conformance, fuzz and memory tests), under
these rules: `no_std` without `alloc`; no `core::fmt` in the hot or resident path;
licence compatible with the project licence (decision D-1 in PLAN.md;
permissive sources only — do not copy code from the GPL Rockbox tree, use the
port for hardware facts); maintained, or small enough to vendor and own.
Expected core set: `embassy-executor`, `embassy-time` (+ driver/queue
crates), `embassy-sync`, `embassy-futures`, `embassy-usb`,
`embassy-usb-driver`, `embedded-hal`/`embedded-hal-async`, `embedded-io-
async`, `embedded-storage-async`, `embedded-graphics`, `heapless`, `defmt`,
`critical-section`, `cortex-m` (without its critical-section feature),
`static_cell`, RustCrypto `hmac`/`sha1`/`sha2`/`aes`/`chacha20poly1305` for
the vault. `cortex-m-rt` is **not** used: the vector table, reset path and
memory layout are specific enough (RAM-only image, SRAM2/3 bring-up before
`.bss`, pager before `main`) that a 100-line own crt0 in `firmware` is
clearer.

## 10. Coding rules that keep it small and fast

1. Measure before making anything resident or raising an optimisation level;
   record the number in the commit.
2. No `f64`. `f32` only where the FPU wins on the device benchmark (it has
   single precision only); fixed point otherwise, written so that LLVM emits
   `smull`/`smlal`/`ssat`.
3. No recursion in firmware crates (stack bound); directory walks and parsers
   are iterative with explicit depth limits.
4. No `unwrap`/`expect`/indexing panics in firmware paths; parsers return
   errors on malformed input (fuzzed).
5. Generic code is fine when paged; the resident set is written by hand with
   concrete types.
6. One owner per peripheral; sharing goes through a task or an async mutex,
   never through `static mut`.
7. Every driver exposes counters (errors, retries, underruns, timeouts)
   through one `Stats` trait; the developer channel and Diagnostics read them.
8. Every `unsafe` block has a `// SAFETY:` line; every register write that is
   copied from vendor behaviour without understanding carries
   `// VERBATIM(vendor: <module>@<address>)` so it can be found and explained
   later.
