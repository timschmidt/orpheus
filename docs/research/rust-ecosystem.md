# Rust embedded ecosystem research for an ATJ2157 MP3-player firmware

Research date: 2026-11-23. Target: Actions ATJ2157 (Cortex-M4F, `thumbv7em-none-eabihf`), 320 KB SRAM, code in RAM (demand-paged from SPI NOR), pure Rust, `no_std`, Embassy.

## How to read this document

- **Versions and dates** come from the crates.io API on 2026-11-23 unless stated. "Date" is the publish date of the listed version.
- **`no_std` / alloc status** was checked in the published crate source (presence of `#![no_std]` in `lib.rs`, Cargo features, and a count of `alloc`/`Vec`/`Box`/`std::` uses). A crate marked "std" has no `no_std` attribute in the version listed. Nothing was compiled for `thumbv7em`; "no_std" here means "declares it", not "builds and fits".
- **RAM figures** are quoted from crate READMEs where a source is given, and are otherwise labelled as estimates.
- Embassy `main` was read at commit `2cc46e558b` (2026-11-23).
- Part of the source material was gathered by helper agents that were cut off before writing up; I re-read their downloaded files myself and only report what I could see in them. Sections 1, 3, 5, 8, 9 and 10 got the most attention; sections 4, 6, 7 are lighter (metadata and README level). Each section ends with what could not be verified.

## The ten findings that change the plan

1. **An official ATJ2157 datasheet is public** (V1.2, 2022-04-30, 336 pages): register descriptions for PMU, CMU, RMU, RTC, DMA, SPI, UART, TWI, DAC/ADC/I2S, LCD, PWM, SARADC and GPIO/MFP, plus the interrupt table and memory map. It gives "up to 288 MHz". It does not document the SD, USB or SPI-cache controller registers. (Section 10)
2. **The datasheet describes a hardware cache for running code directly from SPI NOR** ("Support code runs directly in SPI nor/SRAM", 32 KB I-cache, 4-way, "SPI Cache" controller at `0xC0050000`). If this works on the board it is hardware XIP and could replace the MPU-fault pager. The two odd 32 KB SRAM blocks may be the cache memories. This is a lead to test, not an established fact. (Section 10)
3. **Embassy's released crates and `main` have diverged for eight months.** `main` has a reworked, breaking USB driver trait and new classes including **Mass Storage and MTP**; the last release (2026-03-20) has neither. (Sections 1, 8)
4. **Embassy feature names changed:** `arch-cortex-m` is now `platform-cortex-m`; `task-arena-size-*` no longer exists; stable Rust is enough. (Section 1)
5. **There is now an Embassy-project SD host protocol crate, `sdio` 0.5.3**: implement one `MmcBus` trait for the controller and get card init plus a `block-device-driver` block device. (Section 6)
6. **Allocation-free pure-Rust decoders appeared in the last three months**: `nanomp3-core`, `mbop3`, the `nanoaudio` family (MP3, FLAC, SBC, ADPCM, tags), `opuscule` (Opus). All are young and little used. Vorbis, AAC, Speex, APE, WMA, WavPack and ALAC still have **no** `no_std` pure-Rust decoder. Symphonia is still `std`-only; a `no_std` port is an open PR. (Section 3)
7. **exFAT has a credible `no_std` async crate, `exfat-slim` 0.7.0**; `embedded-sdmmc` is still sync, FAT16/32 only. The async FAT crate `embedded-fatfs` is still git-only. (Section 5)
8. **No pure-Rust Bluetooth Classic host stack exists.** `trouble-host` is BLE-only. `bt-hci` has Classic HCI command types but no H5 transport. (Section 9)
9. **The Bluetooth chip may not be an HCI controller at all.** Reverse-engineering notes for an ATJ2127 player describe a Beken BK3266 audio SoC that runs the whole Bluetooth stack itself and is driven by a proprietary framed UART protocol at 1.5 Mbaud. Identify the chip on the board before planning any host stack. (Sections 9, 10)
10. **A sibling project's notes list SWD on GPIO62/63** for the AGPTEK A02 (same SoC). Unverified for this board, but worth probing. (Sections 10, 11)

## 1. Embassy today

### Recommendation

- Use Embassy on **stable Rust** (current stable is 1.100.0, released 2026-11-12). The `nightly` feature of `embassy-executor` is optional; since `embassy-executor` 0.8.0 (2025-07-31) task storage is statically allocated on stable and **all `task-arena-size-*` features are gone**. Nothing has to be tuned.
- The feature names changed: since `embassy-executor` 0.10.0 the `arch-*` features are called **`platform-*`** (`platform-cortex-m`, `platform-std`, ...). `executor-thread` and `executor-interrupt` are unchanged.
- For a chip without a HAL you write exactly three things that Embassy itself needs: (1) a **time driver** (`embassy-time-driver::Driver`: two functions), (2) interrupt plumbing (vector table + NVIC, from `cortex-m`/`cortex-m-rt`), (3) for USB, an **`embassy-usb-driver`** implementation. Everything else (GPIO, UART, I2C, SD, LCD, DAC) is ordinary driver code that only uses `embassy-sync` wakers/signals.
- **Decide early whether to track the last crates.io release or a pinned git revision of `main`.** The last coordinated release was 2026-03-20 (eight months ago). `main` (checked at commit `2cc46e558b`, 2026-11-23) contains large unreleased, breaking changes that matter here: the USB driver trait was reworked (`ControlPipe` merged into `Bus`, transfer-level endpoint API, `EndpointConfig`), and `embassy-usb` gained MSC, MTP, UAC2, vendor/WinUSB, CCID and per-class Cargo features. Writing the USB driver against 0.2.2 means porting it once the next release lands; writing it against `main` means pinning a git rev. Given that Mass Storage and MTP exist only on `main`, a pinned git rev is the pragmatic choice for the USB crates; the executor/time/sync crates can come from crates.io or from the same rev (do not mix: `embassy-time-driver` and the timer-queue crates use `links=` to forbid two copies).

### Versions (crates.io API, queried 2026-11-23)

| Crate | Latest release | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `embassy-executor` | 0.10.0 | 2026-03-20 (changelog date 2026-03-10) | no_std, no alloc | MIT OR Apache-2.0 | `platform-cortex-m` + `executor-thread` (+ `executor-interrupt`). Optional `scheduler-priority` / `scheduler-deadline` (EDF) since 0.10.0. | https://crates.io/crates/embassy-executor |
| `embassy-executor-macros` | 0.8.0 | 2026-03-20 | proc-macro | MIT OR Apache-2.0 | `#[task]`, `#[main]`. | https://crates.io/crates/embassy-executor-macros |
| `embassy-executor-timer-queue` | 0.1.0 | 2025-08-25 | no_std | MIT OR Apache-2.0 | Holds the per-task `TimerQueueItem`; glue between executor and timer queue. | https://crates.io/crates/embassy-executor-timer-queue |
| `embassy-time` | 0.5.1 | 2026-03-20 | no_std | MIT OR Apache-2.0 | User API (`Instant`, `Timer`, `Ticker`, `with_timeout`). Features `std`, `mock-driver`, `generic-queue-N`, `tick-hz-*`. | https://crates.io/crates/embassy-time |
| `embassy-time-driver` | 0.2.2 | 2026-03-20 | no_std | MIT OR Apache-2.0 | The trait you implement. Trait unchanged since 0.2.0 (2025-01-02). | https://crates.io/crates/embassy-time-driver |
| `embassy-time-queue-utils` | 0.3.2 | 2026-04-21 (0.3.1 yanked) | no_std | MIT OR Apache-2.0 | Ready-made timer queue for drivers. This is the current mechanism. | https://crates.io/crates/embassy-time-queue-utils |
| `embassy-time-queue-driver` | 0.1.0 | 2024-01-11 | - | MIT OR Apache-2.0 | **Obsolete** (pre-2025 mechanism). Do not use. | https://crates.io/crates/embassy-time-queue-driver |
| `embassy-sync` | 0.8.0 | 2026-03-20 | no_std, no alloc | MIT OR Apache-2.0 | `Channel`, `Signal`, `Mutex`, `Pipe`, `PubSub`, `Watch`, `zerocopy_channel`. | https://crates.io/crates/embassy-sync |
| `embassy-futures` | 0.1.2 | 2025-08-27 | no_std, no alloc | MIT OR Apache-2.0 | `select`, `join`, `yield_now`. | https://crates.io/crates/embassy-futures |
| `embassy-usb` | 0.6.0 | 2026-03-20 | no_std, no alloc (`heapless`) | MIT OR Apache-2.0 | Released classes: CDC-ACM, CDC-NCM, CMSIS-DAP v2, DFU, HID, MIDI, UAC1 (speaker), WebUSB. See section 8 for what is only on `main`. | https://crates.io/crates/embassy-usb |
| `embassy-usb-driver` | 0.2.2 | 2026-05-28 (0.2.1 yanked) | no_std | MIT OR Apache-2.0 | Released trait still has `ControlPipe`. `main` is breaking (below). | https://crates.io/crates/embassy-usb-driver |
| `embassy-usb-synopsys-otg` | 0.4.0 | 2026-05-28 | no_std | MIT OR Apache-2.0 | Reference for a HAL-independent driver crate (DWC2). | https://crates.io/crates/embassy-usb-synopsys-otg |
| `embassy-usb-logger` | 0.6.0 | 2026-03-20 | no_std | MIT OR Apache-2.0 | `log` (not defmt) over CDC-ACM. | https://crates.io/crates/embassy-usb-logger |
| `embassy-usb-dfu` | 0.3.0 | 2026-03-20 | no_std | MIT OR Apache-2.0 | DFU 1.1 on top of `embassy-boot`. | https://crates.io/crates/embassy-usb-dfu |
| `embassy-embedded-hal` | 0.6.0 | 2026-03-20 | no_std | MIT OR Apache-2.0 | Shared I2C/SPI bus devices, flash `Partition`, blocking-to-async adapters. | https://crates.io/crates/embassy-embedded-hal |
| `embassy-hal-internal` | 0.5.0 | 2026-03-20 | no_std | MIT OR Apache-2.0 | "DO NOT USE DIRECTLY" but it is what third-party HALs (ch32, hpm, py32) build on: `interrupt_mod!`, `Peri`, `peripherals!`, `atomic_ring_buffer`. Features `cortex-m`, `prio-bits-N`. | https://crates.io/crates/embassy-hal-internal |
| `embassy-boot` | 0.7.0 | 2026-03-20 | no_std | MIT OR Apache-2.0 | See note below; assumes XIP-style ACTIVE/DFU partitions. | https://crates.io/crates/embassy-boot |
| `musb` | 0.3.1 | 2026-08-13 | no_std | Apache-2.0 | Third-party `embassy-usb-driver` (and `usb-device`) implementation for the Mentor MUSB core, kept in a HAL-independent crate. Useful structural prior art; I did not read its control-pipe code. | https://github.com/decaday/musb |

Base crates: `cortex-m` 0.7.10 (2026-10-22), `cortex-m-rt` 0.7.7 (2026-09-24), `critical-section` 1.2.0 (2024-10-16; 1.3.0 was yanked), `heapless` 0.9.4 (2026-11-18), `static_cell` 2.1.1 (2025-06-22), `portable-atomic` 1.15.0 (2026-08-09), `embedded-hal` 1.0.0 and `embedded-hal-async` 1.0.0 (both 2024-01-09), `embedded-io` 0.7.1 / `embedded-io-async` 0.7.0 (2025-09/10).

### What must be implemented for a chip with no HAL

#### Time driver (`embassy-time-driver` 0.2.2; identical on `main`)

```rust
pub trait Driver: Send + Sync + 'static {
    fn now(&self) -> u64;                              // monotonic, never overflows, must not fault before init
    fn schedule_wake(&self, at: u64, waker: &Waker);   // wake `waker` at tick `at`
}
embassy_time_driver::time_driver_impl!(static DRIVER: MyDriver = MyDriver { .. });
```

- The old `allocate_alarm` / `set_alarm_callback` / `set_alarm` API was removed in 0.2.0 (2025-01-02). A driver now owns the timer queue.
- The standard pattern (from the crate docs): keep one `embassy_time_queue_utils::Queue` behind a `critical_section::Mutex<RefCell<..>>`. In `schedule_wake` call `queue.schedule_wake(at, waker)`; if it returns `true`, loop `set_alarm(queue.next_expiration(now))` until the alarm is accepted. In the compare interrupt call `queue.next_expiration(now)` (this wakes expired tasks) and re-arm.
- **Integrated queue (default):** no list of its own; the links live in each task's header (`TimerQueueItem`, 6 words). No capacity limit, but only works with wakers from `embassy-executor`. **Generic queue** (`generic-queue-8/16/32/64/128` on `embassy-time`): fixed-capacity list, works with any executor. Since `embassy-time-queue-utils` 0.3.2 enabling a generic queue disables the integrated one unless `integrated-timers` is also set.
- Tick rate: `tick-hz-*` feature, default 1 MHz. A driver with a fixed rate enables the feature itself. The hardware needs a free-running counter extended to 64 bits in software plus one compare interrupt. This design is tickless by construction: there is no periodic tick, only the next-expiry alarm.
- Reference implementations to copy from: `embassy-nrf/src/time_driver.rs` (24-bit RTC, overflow counting), `embassy-stm32/src/time_driver.rs`. Book page: https://github.com/embassy-rs/embassy/blob/main/docs/pages/developer_time.adoc

#### Executor on Cortex-M

- Features: `platform-cortex-m`, `executor-thread`, optionally `executor-interrupt`. Requires single core, bare metal, privileged mode.
- Thread executor main loop is `poll(); wfe` and the pender is `sev`. Interrupt executors are pended through NVIC (`STIR` on ARMv7-M) and need one otherwise unused IRQ each; a higher-priority interrupt executor pre-empts the thread executor. This is the mechanism for a hard-priority audio task above the UI.
- **Idle hook / custom low power:** there is no callback. The documented way is to wrap `raw::Executor` yourself: define `__pender` and write your own loop (`poll()`, then whatever sleep sequence the SoC needs). The README shows this. On `main` the pender becomes a trait registered with `pender_impl!` (the `__pender` symbol still works), and a `trace` `idle` callback is added.
- **Memory model:** each `#[embassy_executor::task]` gets its own `static` sized exactly for its future (times `pool_size`). On stable this is computed with const-fn size/align helpers; with the `nightly` feature it uses type-alias-impl-trait instead. Out-of-RAM is a link error, not a runtime panic. Per-task header overhead on a 32-bit target is roughly 40 bytes (state, run-queue link, executor pointer, poll fn, 6-word timer item); this figure is my reading of `raw/mod.rs`, not a published number.
- A task's RAM cost is the size of its future, which includes every local held across an `.await`, including large buffers. Check with `cargo size`/`cargo nm --size-sort`; keep big buffers in explicit statics.
- The spawner API changed in 0.10.0: the error is returned when creating the `SpawnToken`, not when spawning.

#### USB driver (`embassy-usb-driver`)

Released 0.2.2 (works with `embassy-usb` 0.6.0):

```rust
pub trait Driver<'a> {
    type EndpointOut: EndpointOut + 'a; type EndpointIn: EndpointIn + 'a;
    type ControlPipe: ControlPipe + 'a; type Bus: Bus + 'a;
    fn alloc_endpoint_out(&mut self, ep_type: EndpointType, ep_addr: Option<EndpointAddress>,
                          max_packet_size: u16, interval_ms: u8) -> Result<Self::EndpointOut, EndpointAllocError>;
    fn alloc_endpoint_in(/* same args */) -> Result<Self::EndpointIn, EndpointAllocError>;
    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe);
}
pub trait Bus {
    async fn enable(&mut self); async fn disable(&mut self);
    async fn poll(&mut self) -> Event;      // Reset, Suspend, Resume, PowerDetected, PowerRemoved
    fn endpoint_set_enabled(&mut self, ep: EndpointAddress, enabled: bool);
    fn endpoint_set_stalled(&mut self, ep: EndpointAddress, stalled: bool);
    fn endpoint_is_stalled(&mut self, ep: EndpointAddress) -> bool;
    fn force_reset(&mut self) -> Result<(), Unsupported>;        // default: Unsupported
    async fn remote_wakeup(&mut self) -> Result<(), Unsupported>;
}
pub trait ControlPipe {
    fn max_packet_size(&self) -> usize;
    async fn setup(&mut self) -> [u8; 8];
    async fn data_out(&mut self, buf: &mut [u8], first: bool, last: bool) -> Result<usize, EndpointError>;
    async fn data_in(&mut self, data: &[u8], first: bool, last: bool) -> Result<(), EndpointError>;
    async fn accept(&mut self); async fn reject(&mut self);
    async fn accept_set_address(&mut self, addr: u8);
}
pub trait Endpoint { fn info(&self) -> &EndpointInfo; async fn wait_enabled(&mut self); }
pub trait EndpointOut: Endpoint { async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError>;  /* + provided read_transfer */ }
pub trait EndpointIn: Endpoint  { async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError>;       /* + provided write_transfer */ }
```

Unreleased `main` (breaking, per `embassy-usb-driver/CHANGELOG.md` "Unreleased"):

- `ControlPipe` is **merged into `Bus`**: `Driver::start()` returns only `Bus`; `Bus::poll()` returns a new `Event::Setup([u8; 8])`; control methods are `control_data_out(buf)`, `control_data_in(data, needs_zlp)`, `control_accept()`, `control_reject()`, `control_accept_set_address(addr)`, `control_max_packet_size()`. `Event::Reset` carries the negotiated `Speed` (`Low/Full/High/Super/SuperPlus`).
- `alloc_endpoint_in/out(&mut self, config: &EndpointConfig)`; `EndpointConfig` has `addr: Option<EndpointAddress>`, `ep_type`, `max_packet_size`, `interval: EndpointInterval`, `buffers` (hint), and SuperSpeed/high-bandwidth fields.
- Endpoints are **transfer-level**: `EndpointOut::read_transfer(buf)` (until buffer full or short packet) and `EndpointIn::write_transfer(buf, needs_zlp)` are the required methods; the packet-level `read`/`write` are gone. `Endpoint` gains required `set_stalled()`/`is_stalled()`/`is_enabled()`; `Bus::endpoint_set_enabled/stalled` became `async`. Isochronous IN has its own `IsoEndpointIn` trait. Host-side traits now live in the same crate.
- The transfer-level API suits a controller with its own DMA much better than the packet-level one: the driver can hand a whole multi-packet buffer to the DMA engine.

How endpoint allocation works (both versions): class constructors call the `Builder`, which calls `alloc_endpoint_*` on the driver **before** `start()`. The driver chooses the endpoint number (or honours a requested address, possible since 0.2.0) and assigns hardware resources (here: FIFOs); it returns `EndpointAllocError` when it runs out. After `start()` nothing can be allocated. `Bus::endpoint_set_enabled` is called on SET_CONFIGURATION; a bus reset disables all non-control endpoints.

Constraints relevant to a FOTG210-like core (analysis from the trait documentation; not tested on hardware):

- **The status stage is always the driver's job.** The stack never issues a zero-length read/write for the status stage. `accept()` means "let the status stage complete" (on this core: set the control-transfer-done bit), `reject()` means STALL EP0, and `data_in(.., last = true)` / `control_data_in` must let the OUT status complete afterwards. A controller that finishes the status handshake by itself once told "done" therefore fits the model directly; nRF USBD is handled the same way in `embassy-nrf`. The ATJ2157 datasheet describes the core as having "four data endpoint and one pair controller endpoint", embedded DMA access to the FIFOs and EHCI-compatible host structures (section 10 of this document).
- **SET_ADDRESS** has a dedicated hook, `accept_set_address(addr)`, precisely because cores differ in ordering. `embassy-nrf` implements it as `accept()` only ("the peripheral handles this"); Synopsys sets the address before the status stage.
- **A new SETUP must abort the current request**: if SETUP arrives during `data_in`/`data_out`/`accept`/`reject`, the driver must return immediately (`EndpointError::Disabled`) so the stack can fetch the new SETUP. On `main` the same holds for reset/suspend, and the event must not be consumed. If the hardware cannot tell "status finished" apart from "new SETUP arrived", the safe implementation is: treat the control-transfer-done write as completion of `accept()` and surface the next SETUP interrupt as a fresh request.
- If the hardware were to ACK a no-data request's status stage before firmware decides (not expected for this core), `reject()` could not be honoured for such requests. Verify on the device with an unsupported request.
- **Four non-control FIFOs is the real product constraint.** Endpoint cost per class: CDC-ACM 3 (interrupt IN + bulk IN + bulk OUT), MSC 2, MTP 3 (bulk IN/OUT + interrupt IN), HID 1 (IN) or 2, UAC1 speaker 1 (iso OUT) or 2 with feedback, vendor/WinUSB 2. So "MSC + CDC-ACM logging" (5) does not fit; "MSC + HID keyboard" (3) or "MSC + vendor bulk IN log pipe" does. Plan USB personalities (storage / audio / debug) rather than one composite device.
- **High speed in 0.6.0:** classes take the max packet size as a parameter (pass 512 for HS bulk) and a device-qualifier descriptor is produced, but there is no other-speed configuration and no speed reported by the driver. `main` adds `Builder::max_speed()`, `other_speed_config_descriptor()` and `Event::Reset(Speed)`. On 0.6.0 a HS-configured device that enumerates at full speed would advertise invalid 512-byte bulk endpoints.
- Compile-time limits: `max-interface-count-N` and `max-handler-count-N` features (default 4 each).

#### `embassy-boot`

`embassy-boot` 0.7.0 swaps an ACTIVE and a DFU partition and assumes the application runs from the ACTIVE partition; hardware glue exists only for nRF, RP and STM32. On this SoC the boot ROM/loader copies code from NOR into RAM, so the partition-swap logic could at most be reused as a library over an `embedded-storage` NOR driver (`FirmwareUpdater`, state partition). Whether that is worth it versus a simple A/B image selector in the first-stage loader is a design decision; nothing in `embassy-boot` handles a RAM-loaded or demand-paged image.

### Published overhead numbers

- Tweede golf, "Async Rust vs RTOS showdown" (2022-01-31, STM32F446, old Embassy): Embassy `.text` 14,272 B and `.data+.bss` 872 B, versus FreeRTOS/C 20,676 B and 5,480 B; RTIC 8,888 B and 392 B. Compiler versions and optimisation flags are not stated. https://tweedegolf.nl/en/blog/65/async-rust-vs-rtos-showdown/
- arXiv 2610.21064 (2026-10-15, CH32V203 RISC-V, 64 KB SRAM): networking static RAM 9,316 B for smoltcp under Embassy versus 40,719 B for lwIP under FreeRTOS. Only the abstract was read. https://arxiv.org/abs/2610.21064
- The Embassy book publishes no size table. Its FAQ advice: `opt-level = "s"`, `lto = true`, `codegen-units = 1`, and on nightly `build-std` with immediate-abort panics to remove `core::fmt`. https://github.com/embassy-rs/embassy/blob/main/docs/pages/faq.adoc

### Could not verify

- No current, official flash/RAM figures for the executor, `embassy-sync` or `embassy-usb` on thumbv7em. Measure on a skeleton build early.
- Whether and when the next coordinated Embassy release (with the new USB driver trait) will be published.
- Any existing Rust driver for the Faraday FOTG210 device core: crates.io search for "fotg210" and "faraday usb" returns nothing.
- The exact behaviour of the SoC's USB core around status-stage timing; the bullet points above are derived from the trait contract only.

## 2. Chip support scaffolding

### Recommendation

- **PAC:** keep register knowledge in hand-written **chiptool YAML**, one file per peripheral block, and generate Rust with `chiptool gen-block` / `gen-common`. This is how `stm32-data` and `ch32-data` work (the latter's README: "most of the chip definitions are manually written"), it needs no SVD, the files diff well as reverse-engineering progresses, and the generated API (fieldset value types, `read`/`write`/`modify`, no ownership singletons) is what all Embassy HALs are written against. chiptool is git-only (not on crates.io), Apache-2.0/MIT, last push 2026-11-19.
- Alternatives, in order of preference: macros in code with `derive-mmio` + `bitbybit` (no codegen step; knurling project) or `tock-registers`; a hand-written SVD fed to `svd2rust` (verbose XML, owned-singleton API that HALs then bypass); raw `core::ptr::read_volatile` wrappers. `device-driver` is aimed at **external** register devices (I2C/SPI) and is a good fit for the FM tuner, accelerometer and any I2C PMU, not for the SoC's MMIO.
- **Runtime:** `cortex-m-rt` 0.7.7 works for a RAM-only image if `memory.x` simply declares the load region as `FLASH` inside SRAM. Relevant features: `set-vtor` (vector table is not at the reset default), `set-sp`, and the new **`skip-data-copy`** (0.7.6, 2026-07-29), which exists exactly for "copy-from-external-device boot" where the loader has already placed `.data`. `#[pre_init]` is deprecated since 0.7.6; write early init in `global_asm!`. A demand pager, however, needs its own linker script: `cortex-m-rt`'s `link.x` knows only `FLASH` and `RAM`, so either supply a full custom script (keeping cortex-m-rt's symbols) or drop `cortex-m-rt` for a small hand-written startup.
- **No prior art was found for demand paging or overlays on Cortex-M in Rust** (crates.io and web search). This part is original work; the C Rockbox implementation is the reference.

### Crates and tools

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `chiptool` | git only | last push 2026-11-19 | host tool | MIT OR Apache-2.0 | Fork of svd2rust. Subcommands: `generate`, `extract-all`, `extract-peripheral`, `transform`, `fmt`, `check`, `gen-block`, `gen-common`. Described by its README as "experimental". | https://github.com/embassy-rs/chiptool |
| `svd2rust` | 0.37.1 | 2025-10-19 | host tool | MIT OR Apache-2.0 | Needs an SVD. `svdtools` 0.5.0 (2025-05-25) patches SVDs from YAML. | https://github.com/rust-embedded/svd2rust |
| `device-driver` | 2.2.0 | 2026-10-21 | no_std | MIT OR Apache-2.0 | **2.0.0 (2026-08-21) replaced the YAML/JSON/TOML/macro-DSL inputs with its own language, DDSL**; CLI renamed `ddc`; LSP + VS Code extension in 2.2.0. MSRV 1.95. For I2C/SPI devices. | https://github.com/diondokter/device-driver |
| `derive-mmio` | 0.8.0 | 2026-07-20 | no_std | MIT OR Apache-2.0 | Derive MMIO accessors on a `#[repr(C)]` struct; pairs with `bitbybit` 2.0.1 (2026-07-31) and `arbitrary-int` 2.2.0. Embassy's own CMSDK timer driver uses this trio. | https://github.com/knurling-rs/derive-mmio |
| `tock-registers` | 0.12.0 | 2026-11-04 | no_std | MIT/Apache-2.0 | Mature macro-based register blocks; now in its own repo. | https://github.com/tock/tock-registers |
| `safe-mmio` | 0.4.0 | 2026-11-12 | no_std | MIT OR Apache-2.0 | Google; aimed at MMU systems. Not obviously better here. | https://github.com/google/safe-mmio |
| `bitfield-struct` | 0.13.0 | 2026-04-04 | no_std | MIT | Plain bitfield structs if hand-rolling. | https://crates.io/crates/bitfield-struct |
| `volatile-register` / `vcell` | 0.2.2 / 0.1.3 | 2023 / 2021 | no_std | MIT OR Apache-2.0 | Old svd2rust-era primitives; stable, unmaintained in practice. | https://crates.io/crates/volatile-register |
| `cortex-m` | 0.7.10 | 2026-10-22 | no_std | MIT OR Apache-2.0 | Three releases in 2026 after a 3.5-year gap. 0.7.8 made `singleton!` require a `critical-section` implementation (breaking soundness fix). Use feature `critical-section-single-core`. Exposes the `MPU`, `SCB`, `NVIC`, `SYST` register blocks. | https://github.com/rust-embedded/cortex-m |
| `cortex-m-rt` | 0.7.7 | 2026-09-24 | no_std | MIT OR Apache-2.0 | Features: `device`, `set-sp`, `set-vtor`, `set-msplim` (ARMv8-M only, not M4), `zero-init-ram`, `paint-stack`, `skip-data-copy`. MSRV 1.85, edition 2024. | https://docs.rs/cortex-m-rt |
| `critical-section` | 1.2.0 | 2024-10-16 | no_std | MIT OR Apache-2.0 | 1.3.0 (2026-06-09) was yanked. | https://crates.io/crates/critical-section |
| `cortex-mpu` | 0.4.0 | 2019-09-13 | no_std | Apache-2.0 | Stale. Program the ARMv7-M MPU through `cortex_m::peripheral::MPU` (RNR/RBAR/RASR) directly. | https://crates.io/crates/cortex-mpu |
| `flip-link` | 0.1.12 | 2025-11-10 | host tool | MIT OR Apache-2.0 | Moves the stack below `.data/.bss` in the `RAM` region so overflow faults. Assumes the standard two-region layout; with four SRAM blocks and code in RAM, place the stack at the bottom of a block yourself, or guard it with an MPU region since the MPU is in use anyway. | https://github.com/knurling-rs/flip-link |
| `cargo-binutils` | 0.4.0 | 2025-08-26 | host tool | MIT OR Apache-2.0 | `cargo size`, `cargo nm --size-sort`, `cargo objdump`. | https://github.com/rust-embedded/cargo-binutils |
| `cargo-bloat` | 0.12.1 | 2024-05-10 | host tool | MIT | Per-function/per-crate `.text` attribution; works on ELF for embedded targets. No release since 2024. | https://github.com/RazrFalcon/cargo-bloat |
| `cargo-call-stack` | 0.1.16 | 2024-10-28 | host tool | MIT OR Apache-2.0 | Static worst-case stack analysis; nightly-only, fragile. Embassy FAQ points to Dirbaio's fork. | https://github.com/Dirbaio/cargo-call-stack |
| `embedded-alloc` | 0.7.0 | 2026-01-03 | no_std | MIT OR Apache-2.0 | If a bounded heap is unavoidable (a decoder that needs `alloc`): fixed arena, LLFF or TLSF. `talc` 5.1.1 (2026-09-09) is an alternative. | https://github.com/rust-embedded/embedded-alloc |

### Placing and ordering code for a pager

Facts and options (the linker options were not test-built here):

- rustc emits one section per function and per static on ELF targets (`.text.<mangled name>`, `.rodata.<..>`), which is what makes `--gc-sections` work for embedded targets; so input sections can be selected individually by a linker script.
- `#[unsafe(link_section = ".text.resident")]` (edition 2024 syntax) pins individual functions or statics. It does not propagate to callees, to generic instantiations in other crates, or to compiler-generated helpers (`core::fmt`, `memcpy`, panics), so it is suited to marking the small always-resident set (fault handler, pager, NOR driver, ISRs, audio path) rather than to classifying everything.
- A linker script can also classify by **object or crate**: `*libcore-*.rlib:(.text .text.*)`-style archive patterns, or patterns on mangled names. With `lto = "fat"` and `codegen-units = 1` everything becomes one object, so archive-name matching stops working; name-pattern matching still works because mangled names contain the crate and module path (e.g. `.text._ZN4core3fmt*` with legacy mangling; the pattern differs under v0 mangling, so fix the scheme with `-C symbol-mangling-version`).
- LLD supports `--symbol-ordering-file=<file>` (one symbol per line; needs per-function sections; `--no-warn-symbol-ordering` silences misses). The bare-metal ARM targets link with `rust-lld` by default, so `-C link-arg=--symbol-ordering-file=order.txt` is the route to profile-driven page clustering: collect the access order from the pager's fault log, emit the symbol list, relink. LLD does not split input sections, so granularity is one function.
- Nightly-only size levers: `-Zbuild-std=core,alloc` with `-Zbuild-std-features=optimize_for_size`; immediate-abort panics (the rustc test-suite now uses `-Zunstable-options -Cpanic=immediate-abort` together with `-Zbuild-std=core`; the Embassy FAQ still documents the older `build-std-features = ["panic_immediate_abort"]` spelling); `-Zlocation-detail=none`; `-Zfmt-debug=none`. All stable builds can use `opt-level = "s"/"z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`.
- ARMv7-M MPU facts that shape a pager: 8 regions (PMSAv7), power-of-two sizes from 32 bytes, 8 sub-region disable bits for regions of 256 bytes or more, `PRIVDEFENA` background map, and `HFNMIENA` controls whether the MPU applies inside HardFault/NMI. A MemManage handler in Rust is an ordinary `#[exception] fn MemoryManagement()`; the faulting address is in `SCB.MMFAR`. The pager and everything it calls must be resident and must not use `async`.
- Interaction with Embassy: a page fault taken inside a task is transparent (it is an exception), but it blocks that priority level for the duration of the NOR read. Anything with a deadline (audio DMA refill, USB) must have its code and data resident. Interrupt-executor tasks run in handler mode; MemManage must have a higher priority than those executors' IRQs or the fault escalates to HardFault.

### Nearest prior art (none does paging)

- Tock (https://github.com/tock/tock) and Hubris (https://github.com/oxidecomputer/hubris): Rust kernels that program the Cortex-M MPU for isolation; useful as MPU-configuration code references.
- Hopter (https://github.com/hopter-project/hopter, last push 2025-04): Rust embedded OS needing a customised compiler; not paging.
- C references for the mechanism: NuttX on-demand paging (https://nuttx.apache.org/docs/latest/components/paging.html), Zephyr demand paging (Cortex-A only).

### Could not verify

- No Rust crate or project implementing MPU-fault-driven code paging or overlays on Cortex-M was found. Absence of evidence from two crates.io searches and one web search; a private or obscure project may exist.
- `--symbol-ordering-file` through `rust-lld` on `thumbv7em-none-eabihf` was not test-built; it is a documented LLD ELF option and should work, but confirm with a ten-line experiment.
- Which nightly introduced `-Cpanic=immediate-abort` and whether the old `panic_immediate_abort` std feature still works was not pinned down.
- `flip-link` behaviour with a RAM-only, multi-region `memory.x` was not tested.

## 3. Audio codecs in pure Rust, `no_std`

### Recommendation

- **MP3:** `nanomp3-core` 0.2.0 (safe-Rust port of minimp3, `no_std`, no allocator, `f32`). `mbop3` 0.1.0 is an alternative tuned for microcontrollers with published stack/heap measurements. Both use single-precision float only, which suits the M4F.
- **FLAC:** `nanoaudio-flac` 0.1.2 (allocation-free; about 32 to 37 KiB for 16-bit stereo, set by the maximum block size).
- **Opus:** `opuscule` 0.2.2 (decoder only, `no_std`, no heap in the decode path, optional fixed-point build, `forbid(unsafe_code)`).
- **WAV / ADPCM, tags:** `nanoaudio-wav`, `nanoaudio-adpcm`, `audio-codec-algorithms`, `nanoaudio-metadata`.
- **SBC (A2DP):** `nanoaudio-sbc` 0.1.1 (encoder and decoder, about 1.9 KiB RAM per its README).
- **Everything else (Vorbis, AAC, Speex, WavPack, APE, WMA, ALAC):** no `no_std` pure-Rust decoder exists today. Each needs a port (a Rust `std` crate made `no_std`, or a translation of the Rockbox/Tremor/Helix fixed-point C code). Plan these as separate, later work items.
- **Caveat that applies to every recommended crate:** they were first published between 2026-06 and 2026-10 and have small download counts. Treat them as unproven: run conformance vectors on the host and fuzz before trusting them, and keep the decoder behind your own trait so a crate can be replaced.
- **Symphonia is not usable**: 0.6.1 (2026-08-13) is `std` throughout. PR #553 "Port symphonia and related codecs to no_std environment" (opened 2026-09-08) is open; on 2026-11-12 the maintainer said he wants a 0.6.2 release first and proposed a `0.6.1-no_std` branch. Even when merged it will need `alloc` (the PR pulls in `hashbrown`, `libm`, `embedded-io`).

### Decoders

| Format | Crate | Version | Date | no_std / alloc | Arithmetic | RAM | Licence | Notes | URL |
|---|---|---|---|---|---|---|---|---|---|
| MP3 | `nanomp3-core` | 0.2.0 | 2026-09-30 | no_std, no alloc | f32 | about 23 KiB state+scratch, plus input window (README of `nanomp3`: "16 KiB is plenty"; `nanoaudio` uses 4 KiB) and one frame of output (1152 stereo samples) | MIT OR Apache-2.0 | Bit-exact safe port of minimp3. Optional `wide` SIMD dep and `layer12` feature are default-on; disable both. | https://github.com/robbie01/nanomp3 |
| MP3 | `nanomp3` | 0.2.0 | 2026-09-30 | no_std; `alloc`/`std` optional | f32 | as above | MIT OR Apache-2.0 | Adds seeking, gapless trim, tag skipping over `Read + Seek`. | same |
| MP3 | `mbop3` | 0.1.0 | 2026-09-27 | no_std, no alloc (`alloc` feature adds `new_boxed`) | f32 | Repo measurements on ESP32-S3: 19.3 KB heap including 6.6 KB of player buffers (so about 13 KB for the boxed decoder), under 3 KB stack | MIT OR Apache-2.0 | minimp3-derived. Stated priorities: stack, memory, CPU, code size. 165 downloads. | https://github.com/boppofun/mbop3 |
| MP3 | `nanoaudio-mp3` | 0.1.1 | 2026-10-15 | no_std, no alloc | f32 | "~23 KiB, plus 4 KiB of input and one frame of output"; flash 46.7 KiB on Cortex-M4 (README) | MIT | Wrapper over `nanomp3-core` in the nanoaudio pipeline API. | https://github.com/AlixANNERAUD/nanoaudio |
| MP3 | `mp3dec` | 0.1.0 | 2026-10-05 | no_std, no alloc, no unsafe | f32 | not stated | Apache-2.0 OR MIT | Another safe minimp3 port (raphlinus). | https://github.com/raphlinus/mp3dec |
| MP3 | `sori-mp3-dec` | 0.1.0 | 2026-11-23 | no_std, `alloc` optional | f32 and f64 | not stated | MIT OR Apache-2.0 | Published today, 0 downloads. | https://github.com/p14jeffwest/sori-mp3-decoder |
| MP3 | `puremp3` | 0.1.0 | 2019-05-26 | std | f32/f64 | - | MIT OR CC0-1.0 | Dead since 2019. | https://github.com/Herschel/puremp3 |
| MP3 | `rmp3`, `minimp3`, `threepm` | - | - | C bindings | - | - | - | Excluded (C). | - |
| FLAC | `nanoaudio-flac` | 0.1.2 | 2026-10-16 | no_std, no alloc | integer | "about 37 KiB with the defaults (4608 samples, stereo)"; "~32 KiB for 16-bit stereo, `i16` blocks"; flash 13.8 KiB | MIT | Keeps one decoded block; lower `MAXIMUM_BLOCK_SIZE` to shrink. | https://github.com/AlixANNERAUD/nanoaudio |
| FLAC | `claxon` | 0.4.3 | 2020-08-09 | std | integer | - | Apache-2.0 | Mature but std (`std::io`) and unmaintained. Portable with moderate work. | https://github.com/ruuda/claxon |
| FLAC | `rusty_flac` | 0.5.2 | 2026-11-23 | no_std with `alloc` | integer + f64 (encoder) | - | Apache-2.0 | Encoder+decoder; allocation-heavy. | https://github.com/Remade-With-Rust/remade_ffmpeg_rs |
| Vorbis | `lewton` | 0.10.2 | 2021-01-20 | std, alloc | f32 | large (codebooks on heap) | MIT OR Apache-2.0 | No release since 2021. | https://github.com/RustAudio/lewton |
| Vorbis | `rustb-vorbis` | 0.2.0 | 2026-11-22 | std | f32 (some f64) | - | MIT OR Unlicense | stb_vorbis port, one week old. | crates.io |
| Vorbis | `symphonia-codec-vorbis` | 0.6.1 | 2026-08-13 | std | f32 | - | MPL-2.0 | - | https://github.com/pdeljanov/Symphonia |
| Opus | `opuscule` | 0.2.2 | 2026-10-20 | no_std (`std` default-on), no heap in decode path | f32 or `fixed-point` feature | not stated | MPL-2.0 | Decoder only. Bit-exact on RFC 8251 vectors in both builds per README. 45k downloads. | https://github.com/bryal/opuscule |
| Opus | `opus-rs` | 0.1.42 | 2026-11-10 | no_std without alloc when default features are off | f32 | "structs are large" in no-alloc mode; put in a `static` | BSD-3-Clause | Encoder and decoder. About 250 `unsafe` uses. | https://github.com/restsend/opus-rs |
| Opus | `mousiki` | 0.2.1 | 2026-03-26 | no_std + alloc | float or `fixed_point` | - | MIT | Full libopus port (119k lines). | https://github.com/cijiugechu/mousiki |
| Opus | `opus-decoder` 0.1.1, `symphonia-opus` 0.1.3, `ropus` 0.12.18, `unsafe-libopus` 0.2.0 | - | 2024 to 2026 | std | mixed | - | various | Not no_std. | - |
| Opus | `opus-embedded` | 0.1.1 | - | wraps C (`opus-embedded-sys`) | - | - | BSD-3-Clause | Excluded (C). | - |
| Speex | `oxideav-speex` | 0.0.12 | 2026-11-08 | std, alloc | f64-heavy | - | MIT | The only pure-Rust Speex found. Not suitable as is. | https://github.com/OxideAV |
| AAC-LC | `symphonia-codec-aac` | 0.6.1 | 2026-08-13 | std | f32 | - | MPL-2.0 | LC only. Smallest code base (3.1k lines) to port. | Symphonia |
| AAC | `rusty_aac` 2.1.5, `fdk-aac-rs` 0.1.0, `soundkit-aac-lc` 0.12.3, `filmcraft-aac` 0.9.0, `oxideav-aac` 0.1.7 | - | 2026 | std | mixed, f64 common | - | Apache-2.0 / FDK licence / MIT | HE-AAC exists in `rusty_aac` and `fdk-aac-rs`. None is no_std. | crates.io |
| WavPack | `wavicle` | 0.1.0 | 2026-07-18 | std (no deps, one `std::` use) | integer | - | MIT OR Apache-2.0 | Closest to portable. | https://github.com/mark-ik/wavicle |
| WavPack | `symphonia-codec-wavpack` | 0.1.1 | 2026-09-06 | std | integer | - | **GPL-3.0-or-later** | Third-party, not part of Symphonia proper. | crates.io |
| APE | `ape-decoder` 0.3.2, `oxideav-ape` 0.0.5 | - | 2026 | std, alloc | integer | high-compression APE needs large filter histories | MIT OR Apache-2.0 / MIT | - | crates.io |
| WMA | `oxideav-wma` | 0.0.5 | 2026-11-07 | std | f64-heavy | - | MIT | Early. Only candidate found. | crates.io |
| ALAC | `symphonia-codec-alac` | 0.6.1 | 2026-08-13 | std | integer | - | MPL-2.0 | 671 lines; easy port. `alac` 0.5.0 (2018) is std and stale. | Symphonia |
| WAV/ADPCM | `nanoaudio-wav` 0.1.2, `nanoaudio-adpcm` 0.1.2 | - | 2026-10-16 | no_std, no alloc | integer | 40 bytes (IMA state) | MIT | IMA and MS ADPCM, encoder and decoder. | nanoaudio |
| ADPCM/G.711 | `audio-codec-algorithms` | 0.8.1 | 2026-08-18 | no_std | integer | negligible | 0BSD OR Apache-2.0 | A-law, mu-law, IMA ADPCM. 3.3M downloads. | crates.io |
| WAV | `hound` 3.5.1 (std), `wavv` 0.2.1 (no_std + alloc) | - | 2023 / 2025 | - | - | - | Apache-2.0 | A WAV header parser is trivial to hand-write. | - |
| MOD/XM/S3M/IT | `xmrsplayer` + `xmrs` | 0.15.0 / 0.15.2 | 2026-07 | no_std + alloc (heavy) | fixed-point mixer | whole module is deserialised into heap structures; too large for big modules | MIT | Only realistic for small MODs with a bounded heap. | https://codeberg.org/sbechet/xmrs |
| Ogg container | `ogg` | 0.9.2 | 2025-01-12 | std | - | - | BSD-3-Clause | Hand-write a streaming page reader instead (small). | https://github.com/RustAudio/ogg |
| Ogg Opus | `oggopus-embedded` | 0.1.1 | 2025-05-19 | no_std, no alloc | - | - | BSD-3-Clause | Parses from in-memory slices only. | crates.io |
| MP4/M4A | `mp4-core` | 0.3.2 | 2026-10-25 | no_std + alloc | - | - | MIT OR Apache-2.0 | Box reader/writer, metadata. New. | crates.io |
| Tags | `nanoaudio-metadata` | 0.1.1 | 2026-10-15 | no_std, no alloc | - | small | MIT | ID3, FLAC, RIFF, AIFF, MP3 VBR headers. | nanoaudio |
| Tags | `id3-core` | 0.3.1 | 2026-10-25 | no_std + alloc | - | - | MIT OR Apache-2.0 | - | crates.io |

`rockbox-codecs` 0.2.2 / `rockbox-dsp` / `rockbox-metadata` on crates.io are Rust wrappers that build Rockbox's C code (GPL-2.0-or-later, std); excluded as C, but they show the Rockbox fixed-point codecs are the obvious porting source for the missing formats.

### Encoders

| Purpose | Crate | Version | Date | no_std / alloc | Licence | Notes |
|---|---|---|---|---|---|---|
| WAV / IMA ADPCM recording | `nanoaudio-adpcm`, `audio-codec-algorithms` | 0.1.2 / 0.8.1 | 2026 | no_std, no alloc | MIT / 0BSD OR Apache-2.0 | The safe default for voice and FM recording. |
| FLAC | `flacenc` | 0.5.1 | 2025-12-18 | std | Apache-2.0 | Not usable as is. A fixed-predictor + Rice FLAC encoder is small enough to write by hand. |
| MP3 | `shine-rs` | 0.1.5 | 2026-09-23 | std, alloc | LGPL-2.0 | Port of the fixed-point shine encoder; needs a no_std port. `audio2mp3-core` 0.2.0 (MIT OR Apache-2.0, std, f32) is an alternative. |
| Opus | `opus-rs` 0.1.42, `mousiki` 0.2.1 | - | 2026 | no_std (opus-rs without alloc) | BSD-3-Clause / MIT | Encoder state is large; CPU cost at 48 MHz not measured. |
| Speex | none | - | - | - | - | `oxideav-speex` (std) only. |
| SBC | `nanoaudio-sbc` | 0.1.1 | 2026-10-15 | no_std, no alloc | MIT | Encoder and decoder incl. mSBC; README: 1.9 KiB RAM, 8.4 KiB flash on Cortex-M4. `mini_sbc` 0.1.7 (2023) is decoder-only. |
| LC3 | `lc3-codec` | 0.2.0 | 2022-06-12 | no_std (`alloc` default-on) | Apache-2.0 | README: decoder 27,564 bytes of working RAM; 10 ms frame decodes in 5.4 ms on nRF52840. `lc3` 0.2.0 (2026-11-21) is std. |
| AAC | none no_std | - | - | - | - | - |

### What fits in 30 to 60 KB of RAM today

1. WAV, G.711, IMA/MS ADPCM: negligible RAM. Available now.
2. SBC encode/decode: about 2 KiB. Available now.
3. MP3: about 23 KiB plus a 4 to 16 KiB input window plus output frame. Available now.
4. FLAC 16-bit stereo: 32 to 37 KiB; 24-bit or 8 kHz-block files need more. Available now.
5. Opus decode: fits in principle (reference decoders need on the order of 20 to 30 KB for stereo; this figure is from memory, not measured for `opuscule`). Measure before committing.
6. AAC-LC: fits in principle (C fixed-point decoders such as Helix use roughly 20 to 30 KB; from memory). Needs a port.
7. Vorbis: marginal. Codebook and floor/residue setup is allocated per stream and commonly exceeds 60 KB with float decoders; the low-memory Tremor variant used by Rockbox is the realistic model. Needs a port.
8. WavPack, ALAC: fit after a port (small state). APE: only the lower compression levels are realistic in CPU and RAM. WMA: needs a port of a fixed-point decoder; RAM comparable to AAC. Speex: small state, needs a port. MOD: only with a heap and size limits.

### Could not verify

- Any of the recommended crates building for `thumbv7em-none-eabihf` and their real code size there (only `nanoaudio` publishes Cortex-M4 flash figures).
- RAM use of `opuscule`, `opus-rs`, `mp3dec`, `sori-mp3-dec` (not stated in their READMEs).
- Real-time margins at 48 MHz. The only device timing found is `mbop3` on a 240 MHz ESP32-S3 (about 7.5% of a core for 48 kHz mono), which suggests MP3 at 48 MHz on an M4F is feasible but not by how much.
- The estimates in items 5 to 8 of the list above are from general knowledge of the C decoders, not from measurement.
- Whether `nanoaudio-sbc` really supports all A2DP SBC modes (joint stereo, 8 subbands, 16 blocks, loudness/SNR allocation); taken from the crate description.

## 4. DSP

Researched at metadata and README level only.

### Recommendation

- **Rule one: no `f64` anywhere in the audio path.** The M4F FPU is single precision; every `f64` operation becomes a soft-float library call. Several otherwise attractive crates use `f64` internally (`bs2b`, `dasp_interpolate`, `rubato`, `simple_src`).
- **Volume, mixing, format conversion, simple resampling:** `nanoaudio-dsp` 0.1.1 (allocation-free, fixed-point or `f32` selectable). Young; verify quality of its resampler by measurement.
- **EQ / biquads:** `biquad` 0.6.0 for `f32` (DF1 and DF2T, coefficient design included), or `idsp` 0.23.1 for integer biquads with proper fixed-point handling (QUARTIQ, used in lab instruments, actively maintained).
- **Resampling:** there is no established `no_std`, allocation-free, high-quality resampler. Options are `nanoaudio-dsp`, `dasp_interpolate` (generic, sinc/linear, `f64` in places), `resampler` 0.5.2 (`no_std` feature but needs `alloc`, about 230 `unsafe` uses), or porting the 1,300-line `speexdsp-resampler` (std today). If the DAC can run at both 44.1 kHz and 48 kHz families natively (the datasheet lists 8 to 96 kHz), most resampling can be avoided altogether.
- **Fixed-point types and math:** `fixed` 1.31.0, `cordic` 0.1.5, `micromath` 2.2.0 (fast `f32` approximations), `libm` 0.2.16.
- **Spectrum display:** `microfft` 0.6.0. Select the smallest `size-N` feature (the sine table is sized by the largest enabled FFT; default is 4096) and use the real FFT (`rfft_256` or `rfft_512`) on a stack buffer.
- The SoC has an "FFT/FIR/IIR hardware engine" according to the datasheet feature list, but the datasheet has no register chapter for it (section 10).

### Crates

| Name | Version | Date | no_std / alloc | Arithmetic | Licence | Notes | URL |
|---|---|---|---|---|---|---|---|
| `nanoaudio-dsp` | 0.1.1 | 2026-10-15 | no_std, no alloc | fixed or f32 (some f64 in source: 26 mentions, not checked whether on hot paths) | MIT | PCM conversion, volume, mixing, resampling, pipelines. 152 downloads. | https://github.com/AlixANNERAUD/nanoaudio |
| `biquad` | 0.6.0 | 2026-03-22 | no_std | f32 / f64 generic | MIT OR Apache-2.0 | Coefficients for LP/HP/peaking/shelf; DF1, DF2T. | https://github.com/korken89/biquad-rs |
| `idsp` | 0.23.1 | 2026-10-17 | no_std | mostly integer | MIT OR Apache-2.0 | Biquad/IIR with fixed-point, half-band filters, PLL, CORDIC-style trig. Depends on `dsp-fixedpoint` 0.2.2 and `dsp-process` 0.3.2. | https://github.com/quartiq/idsp |
| `fixed` | 1.31.0 | 2026-03-20 | no_std | fixed | MIT/Apache-2.0 | The standard Q-format types. | https://gitlab.com/tspiteri/fixed |
| `cordic` | 0.1.5 | 2021-05-16 | no_std | fixed | BSD-3-Clause | sin/cos/atan/sqrt/exp on `fixed`. | crates.io |
| `fixed-sqrt` 0.3.0, `fixed_trigonometry` 0.4.4, `fixed-filters` 0.1.0 | - | 2023 or older | no_std | fixed | Apache-2.0 / MIT | Small helpers; low activity. | crates.io |
| `micromath` | 2.2.0 | 2026-11-21 | no_std | f32 | Apache-2.0 OR MIT | Small approximate math; good for UI/meters, not for filter design. | crates.io |
| `libm` | 0.2.16 | 2026-01-24 | no_std | f32/f64 | MIT | Exact math when needed. | crates.io |
| `arm-dsp` | 0.1.0 | 2026-11-06 | no_std | fixed | MIT OR Apache-2.0 | Safe wrappers for SMMUL/QADD/SSAT with portable fallbacks. New, unproven. | crates.io |
| `embedded-dsp` | 0.6.0 | 2026-09-16 | no_std, no alloc | f32 + fixed | MIT OR Apache-2.0 | Large (24k lines) new all-in-one library: filters, dither, dynamics. Unproven. | https://github.com/leftger/embedded-dsp |
| `microfft` | 0.6.0 | 2024-04-14 | no_std, no alloc | f32 | MIT | In-place radix-2, sizes 4 to 32768, real and complex. Sine table in flash. | https://gitlab.com/teskje/microfft-rs |
| `embfft` | 0.2.1 | 2025-04-09 | no_std, no alloc | f32 | MIT | "Non-blocking" FFT that can be stepped incrementally. | crates.io |
| `kofft` | 0.1.5 | 2025-08-14 | no_std but uses alloc/hashbrown | f32 | MIT OR Apache-2.0 | Not allocation-free despite its description. | crates.io |
| `rustfft` 6.4.1, `realfft` 3.5.1 | - | 2025 / 2026 | std | - | MIT OR Apache-2.0 / MIT | Not usable. | - |
| `rubato` | 5.1.0 | 2026-11-12 | std | f64-heavy | MIT OR Apache-2.0 | The usual desktop choice; not usable. | - |
| `resampler` | 0.5.2 | 2026-10-10 | `no_std` feature, needs alloc | f32 | Apache-2.0 OR MIT | README: "supports no-std environments with alloc". | https://github.com/hasenbanck/resampler |
| `dasp` family | 0.11.0 | 2020-05-29 | no_std (features) | generic, f64 in interpolators | MIT OR Apache-2.0 | No release since 2020. | https://github.com/RustAudio/dasp |
| `fon` | 0.6.0 | 2022-01-29 | no_std + alloc | f32 | Apache-2.0 OR MIT OR BSL-1.0 | Stale. | crates.io |
| `speexdsp-resampler` | 0.1.0 | 2020-02-14 | std (small) | f32 (some f64) | BSD-3-Clause | 1,313 lines; a port candidate. | crates.io |
| `bs2b` | 0.2.2 | 2026-07-13 | no_std option | **f64** | MIT | Crossfeed. Rewrite in f32/fixed rather than use. | crates.io |
| `fundsp` | 0.23.0 | 2026-01-07 | no_std + alloc | f32/f64 | MIT OR Apache-2.0 | Synthesis graph library; too heavy. | - |

### Could not verify

- Quality (aliasing, passband) of any `no_std` resampler listed.
- Flash cost of `microfft` tables per size (the README says the table "claims a considerable amount of memory" without a number in the part read).
- The hardware FFT/FIR/IIR engine: existence is claimed in the datasheet feature list only.

## 5. Filesystems

### Recommendation

- **Standardise on the `block-device-driver` 0.2.0 trait** as the seam between the SD driver and filesystems. It is async, takes slices of aligned 512-byte blocks (so one call maps to one CMD18/CMD25 multi-block transfer), and is what `embedded-fatfs`, `exfat-slim`, the Embassy `sdio` crate and embassy-usb's MSC adapter all speak.
- **FAT12/16/32:** `embedded-fatfs` (MabezDev's async fork of `rust-fatfs`): LFN, Unicode, write, async. It is **git-only**: the crates.io entry is a 0.0.0 placeholder from 2023, while the repository has version 0.1.0 and was last pushed 2026-10-23 (seek fixes, spec-compliance fixes). A published fork exists as `deluge-embedded-fatfs` 0.2.0 (2026-11-06).
- **exFAT:** `exfat-slim` 0.7.0 (2026-08-29) is the only candidate that is `no_std`, allocator-optional, async, read/write, and built on `block-device-driver`. It is young; test it hard (power-fail, fragmentation, large directories) against a host implementation.
- **Do not use `embedded-sdmmc`** for this design: 0.10.0 (2026-08-10) is still synchronous, FAT16/FAT32 only, no exFAT, and its `SdCard` type is SPI-mode only.
- **Fallback that avoids exFAT entirely:** format the 256 GB card as FAT32 (valid up to 2 TiB with 512-byte sectors; 4 GiB file limit). The stock firmware ships exFAT drivers (`exfatn.drv`, `exfats.drv` appear in its RAM image), so users will expect exFAT cards to work, but FAT32-only is a legitimate first milestone.
- Two filesystem crates means two code bases in a RAM-constrained image. Hide both behind one small VFS trait of your own (open, read, seek, readdir cursor, write, rename, delete) so the UI and decoders do not depend on either.

### Crates

| Name | Version | Date | no_std / alloc | Async | exFAT | Licence | Notes | URL |
|---|---|---|---|---|---|---|---|---|
| `embedded-fatfs` | 0.1.0 in git (crates.io: 0.0.0 placeholder, 2023-07-16) | repo pushed 2026-10-23 | no_std; `alloc` optional; features `lfn`, `unicode` | yes | no | MIT | FAT12/16/32, LFN, write. Changelog: `Dir` methods iterative instead of recursive; `FileContext` can be saved and restored "to avoid costly seek operations". I/O via `embedded-io-async`; `BufStream`/`StreamSlice` adapters bridge to `block-device-driver`. Repo also holds `block-device-driver`, `block-device-adapters`, `embedded-partitions`, `sdspi`. | https://github.com/MabezDev/embedded-fatfs |
| `deluge-embedded-fatfs` | 0.2.0 | 2026-11-06 | same | yes | no | MIT | Fork "with upstream fixes applied", maintained for another project. | https://github.com/FirestormAudio/deluge-sdk |
| `exfat-slim` | 0.7.0 | 2026-08-29 | no_std; `alloc` optional; no unsafe | async and blocking | **yes** | Apache-2.0 | Read, write, create, append, truncate, seek, rename, delete, directory listing, timestamps; block caching for bitmap, FAT chain and data; `embassy` feature (actor pattern so the block device can be shared). TODO per README: zero-fill past valid-data-length, preallocating truncate. | https://github.com/ninjasource/exfat-slim |
| `embedded-exfat` | 0.4.0 | 2025-11-04 | no_std option (`std` default) | via `maybe-async` | yes | MIT | Older design, 46 `unsafe` uses, fixed max filename feature. | https://github.com/qiuchengxuan/exfat |
| `exfat-embedded` | 0.1.3 | 2026-09-07 | no_std, no alloc, no deps | not checked | yes | MIT OR Apache-2.0 | "Allocation-free exFAT filesystem primitives": building blocks, not a full FS API. | https://github.com/joebnb/exfat-embedded |
| `unifat` | 0.2.1 | 2026-10-12 | no_std option (`std` default); uses alloc | sync (`embedded-io`) | yes (plus FAT16/32, one API) | MPL-2.0 | Attractive idea (one crate for both), but synchronous and allocation use not assessed. | https://github.com/inex-rs/unifat |
| `hadris-fat` | 2.5.0 (3.0.0-rc.4 exists) | 2026-10-03 | no_std option | `async` feature | "unstable-exfat" feature | MIT | Large (17k lines), std-first. | crates.io |
| `lamexfat` | 0.1.2 | 2026-10-16 | no_std | no | read-only | MIT/Apache-2.0 | For UEFI loaders. | crates.io |
| `embedded-sdmmc` | 0.10.0 | 2026-08-10 | no_std, no alloc | **no** | no | MIT OR Apache-2.0 | FAT16/32. 0.10.0 added `open_long_name_file_in_dir` and early exit from the directory callback; card types moved to `embedded-sdmmc-types` 0.1.0. Block trait: `read(&self, blocks: &mut [Block], start)`, `write`, `num_blocks` (multi-block capable, sync). | https://github.com/rust-embedded-community/embedded-sdmmc-rs |
| `fatfs` | 0.3.6 | 2023-01-17 | no_std option | no | no | MIT | The upstream of `embedded-fatfs`; no release since 2023. | https://github.com/rafalh/rust-fatfs |
| `resident-fat` | 0.3.1 | 2026-10-16 | no_std + alloc | - | no | MIT OR Apache-2.0 | Keeps the whole FAT and directories in RAM; wrong trade-off here. | crates.io |
| `polyfs` | 0.1.1 | 2026-04-25 | no_std, no alloc | yes | no | GPL-3.0-or-later | Minimal FAT32. Licence. | crates.io |
| `block-device-driver` | 0.2.0 | 2024-03-30 | no_std | yes | - | MIT | `trait BlockDevice<const SIZE: usize> { type Error; type Align; async fn read(&mut self, block_address: u32, data: &mut [Aligned<Self::Align, [u8; SIZE]>]); async fn write(..); async fn size(&mut self) -> Result<u64, _>; }` | in the embedded-fatfs repo |
| `block-device-adapters` | 0.2.0 | 2024-09-11 | no_std | yes | - | MIT | Byte-level stream over a block device; partition slicing. | same |
| `fatfs-embedded`, `littlefs2` | - | - | C bindings | - | - | - | Excluded (C). | - |

### Points that matter for this device

- **Block addressing:** `block-device-driver` uses a `u32` block address, so 512-byte blocks reach 2 TiB. A 256 GB card is fine.
- **Alignment:** the trait carries an `Align` type, so the SD driver can demand DMA-aligned buffers without copies. Note the measured quirk that SRAM2/SRAM3 have different DMA addresses; DMA buffers need to live in a known block.
- **Large directories:** both FAT crates iterate directories from disk (the `rust-fatfs` design is a streaming iterator; `exfat-slim` advertises "list all directory entries"). Neither offers a persistent, seekable directory cursor that survives across UI frames as a public feature that I verified; a music browser over thousands of entries will need its own index (entry number or cluster/offset bookmarks, or a database file built once per card change).
- **Seek cost:** FAT seek is O(chain length). `embedded-fatfs` offers `FileContext` save/restore; exFAT's contiguous-file flag (`NoFatChain`) makes seeks O(1) for unfragmented files, which is the common case for music.
- **Free-space and allocation on huge FAT32:** a 256 GB FAT32 volume with 32 KiB clusters has about 8 million FAT entries (32 MB of FAT). Scanning it for free clusters or free-space counts is slow; rely on FSInfo and avoid full scans.
- **Partition tables:** `block-device-adapters` and `embedded-partitions` (in the embedded-fatfs repo) cover MBR-style slicing. GPT support was not checked.
- **exFAT licensing:** Microsoft published the exFAT specification in 2019 and made its exFAT patents available to Open Invention Network members for Linux. Whether that covers a non-Linux firmware is a legal question I cannot answer; for a hobby firmware it is commonly ignored, for a product it needs advice.

### Could not verify

- Whether an async `embedded-sdmmc` branch exists or is planned (only the release was checked).
- `exfat-slim` behaviour on large (256 GB) volumes, its RAM use, and its directory-iteration API shape.
- Whether `embedded-fatfs` directory iteration can resume from a saved position.
- GPT support in any of the partition helpers.
- The exFAT patent position (stated from memory, not re-checked today).

## 6. SD host

Researched at metadata and README level, plus the trait definitions.

### Recommendation

- **Use the Embassy project's `sdio` crate (0.5.3, 2026-10-19).** It defines `MmcBus`, described in its README as "the lowest-level hardware abstraction for SD/MMC/SDIO host controllers. It corresponds to the Linux `mmc_host_ops` interface", and it contains the card-level logic on top (the README: "a standard implementation of common logic across all devices based on the SDIO standard"; "intended to be a replacement for the now-abandoned `sdio-host` project"). A struct in the crate implements `block_device_driver::BlockDevice`. So the work for this SoC is one `MmcBus` implementation over the SD controller at `0xC0160000` plus its DMA channel.
- `MmcBus` methods (async, generic over a command type): `send_command`, `read_blocks`, `write_blocks`, `read_bytes`, `write_bytes`, `init_idle(hz)`, `tune_bus`, plus bus-setting methods not listed here. Read the trait before designing the driver.
- `embedded-hal` 1.0.0 and `embedded-hal-async` 1.0.0 (both 2024-01-09, unchanged since) contain **no** SD/MMC/SDIO traits. The embedded-hal workspace today holds `embedded-hal`, `-async`, `-bus`, `-nb`, `embedded-can`, `embedded-io`, `embedded-io-async`, `embedded-io-adapters`.
- The SD controller registers are not in the public datasheet; the only public register notes are in the A02-OS hardware document (section 10).

### Crates

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `sdio` | 0.5.3 | 2026-10-19 | no_std | MIT OR Apache-2.0 | `MmcBus` trait + SD/eMMC/SDIO protocol + `BlockDevice` impl. README says current targets are STM32 and ESP32; `esp-hal` depends on `sdio` 0.5. Deps: `aligned`, `block-device-driver`, `embedded-hal(-async)`. | https://github.com/embassy-rs/sdio |
| `sdio-host` | 0.9.0 | 2022-07-27 | no_std, no deps | MIT OR Apache-2.0 | Command/response/CSD/CID/SCR/OCR types only. Called "now-abandoned" by the `sdio` README. | https://github.com/jkristell/sdio-host |
| `embedded-sdmmc-types` | 0.1.0 | 2026-08-10 | no_std | MIT OR Apache-2.0 | Types split out of `embedded-sdmmc` 0.10 (CSD v1/v2/v3 with CRC7 check, block types). | embedded-sdmmc-rs repo |
| `embedded-hal-sdmmc` | 0.1.0-alpha.3 | 2025-06-10 | no_std | MIT OR Apache-2.0 | Experimental host/device traits on Codeberg, "intention is to upstream these traits into embedded-hal after stabilization". Pre-release only. | https://codeberg.org/weathered-steel/embedded-hal-sdmmc |
| `sdmmc-core` | 0.5.0 | 2025-03-23 | no_std | **GPL-3.0-only** | Very complete register/command type library (38k lines). Licence rules it out for a permissively licensed firmware. | crates.io |
| `sdio-host2` | 0.1.7 | 2026-08-20 | no_std | Apache-2.0 | Transaction traits for OS kernels (`dma-api`). Not aimed at MCUs. | crates.io |
| `sdmmc` | 0.1.0 | 2026-01-28 | no_std | GPL-3.0-or-later OR Apache-2.0 OR MIT | ARM64 platform driver. Not relevant. | crates.io |

### Could not verify

- The full `MmcBus` surface and whether it assumes DMA-style whole-transfer semantics that match this controller.
- Whether `embassy-stm32` has already switched its SDMMC driver to `sdio` (its `Cargo.toml` was downloaded but no `sdio` dependency line was seen in the part inspected).
- UHS/1.8 V signalling support (the datasheet says the pads support 3.1 V and 1.8 V; high-speed 50 MHz is the realistic target).

## 7. Display and UI

Researched at metadata and README level only.

### Recommendation

- **Graphics core:** `embedded-graphics` 0.8.2 (2026-02-15). It needs no framebuffer: primitives, text and images stream pixels into a `DrawTarget`. For this panel implement `DrawTarget` (with `fill_contiguous` and `fill_solid` overrides) straight on the SoC's LCD controller, which is a FIFO-based 8080 host, not GPIO bit-banging. Then decide per screen between direct drawing and a 32 KB RGB565 framebuffer in one of the 32 KB SRAM blocks.
- **Panel driver:** `mipidsi` 0.10.0 has an ST7735S model and, since it dropped `display-interface`, its own `Interface` trait (with `ParallelInterface` for GPIO buses and `SpiInterface`). Implementing `Interface` for the LCD controller is about 50 lines and gives init sequences, orientation and windowing for free. It is synchronous; at 128x128 that is acceptable. Async alternatives: `lcd-async` 0.1.3 (async fork of mipidsi) and `display-driver` 0.1.1 with `display-driver-st7735` 0.1.0. The vendor's exact init table and panel offsets are already known from reverse engineering, so a 100-line hand-written ST7735S driver is equally reasonable.
- **Fonts:** this is the hard part for a music player (CJK tags). Compiled-in font crates put glyph data in `.rodata`, which on this device is paged from NOR, so size is a NOR budget question rather than RAM. `u8g2-fonts` 0.9.1 gives the widest ready-made selection. The alternative with the best coverage per byte is the vendor's own approach: a bitmap Unicode font file on NOR or SD read glyph-by-glyph on demand (the `UNICODE.FON` layout is documented in the leaked US212A SDK and in a community toolkit, section 10); no crate does this.
- **Text layout:** `embedded-text` 0.7.3 for wrapping and alignment. No bidi or shaping; Arabic/Indic shaping (`rustybuzz`/`harfrust`) is out of reach in this RAM.
- **Images:** BMP via `tinybmp`; JPEG cover art via a TJpgDec port (`tjpgdec-rs` or `tjpgd-rs`): 3 to 10 KB workspace, row-callback output with 1/2/4/8 downscaling, baseline only. Progressive JPEG and large PNG cover art are not feasible in this RAM and should be rejected gracefully.
- **UI framework:** hand-rolled screens on `embedded-graphics` + `embedded-layout` + `embedded-text`, possibly `embedded-menu` for settings lists. The declarative frameworks either need a heap of unknown size (`slint`, `ratatui` + `mousefood`) or are touch-first (`kolibri`); `buoyant` is the one to evaluate if a declarative style is wanted.

### Crates

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `embedded-graphics` / `-core` | 0.8.2 / 0.4.1 | 2026-02-15 | no_std, no alloc | MIT OR Apache-2.0 | First release since 2023-08. | https://github.com/embedded-graphics/embedded-graphics |
| `embedded-graphics-framebuf` | 0.5.0 | 2023-07-17 | no_std | MIT | Framebuffer `DrawTarget` over a caller buffer. | crates.io |
| `mipidsi` | 0.10.0 | 2026-02-17 | no_std (optional `heapless` for `batch`) | MIT | Models include `st7735s`. Own `Interface` trait; sync. | https://github.com/almindor/mipidsi |
| `lcd-async` | 0.1.3 | 2026-02-26 | no_std | MIT | "Async fork of mipidsi crate"; has parallel and SPI interface modules. | https://github.com/okhsunrog/lcd-async |
| `display-driver` + `display-driver-st7735` | 0.1.1 / 0.1.0 | 2026-09-03 / 2026-02-06 | no_std, no alloc | Apache-2.0 | Async display driver framework; new. | crates.io |
| `st7735-lcd` | 0.10.0 | 2024-03-14 | no_std | MIT | SPI only. | crates.io |
| `display-interface` / `-parallel-gpio` | 0.5.0 / 0.7.0 | 2024-01-12 | no_std | MIT OR Apache-2.0 | No release since 2024-01; `mipidsi` no longer uses it. | https://github.com/therealprof/display-interface |
| `u8g2-fonts` | 0.9.1 | 2026-11-22 | no_std, no alloc | crate MIT OR Apache-2.0; **fonts under their own licences** (see U8g2 LICENSE) | Renderer for the U8g2 font collection on embedded-graphics. | https://github.com/Finomnis/u8g2-fonts |
| `mplusfonts` | 0.3.4 | 2026-03-31 | no_std, no alloc | MIT OR Apache-2.0 | M+ bitmap fonts (kana/kanji) generated by macro for the characters used. Suits fixed UI strings, not arbitrary tags. | crates.io |
| `unifont` | 1.1.0+data-15.1.05 | 2024-04-24 | not checked | MIT (crate) | GNU Unifont data as a crate. Font licence not checked. | crates.io |
| `embedded-graphics-unicodefonts` | 0.2.0 | 2025-09-06 | not checked | MIT | Used by `mousefood`. | crates.io |
| `embedded-ttf` | 0.2.2 | 2025-08-26 | no_std + alloc (`rusttype`) | MIT | TTF rasterising on device: heap and CPU cost; not for CJK at this size. | crates.io |
| `fontdue` | 0.9.5 | 2026-10-19 | no_std + alloc | MIT OR Apache-2.0 OR Zlib | Same remark. | crates.io |
| `bdf-parser` | 0.1.0 | 2021-02-27 | host tool | MIT OR Apache-2.0 | `eg-bdf` and `eg-font-converter` are **not on crates.io** (git only, in the embedded-graphics `bdf` repo). | https://github.com/embedded-graphics/bdf |
| `embedded-text` | 0.7.3 | 2025-10-10 | no_std | MIT | TextBox with wrapping, alignment, optional ANSI. | https://github.com/embedded-graphics/embedded-text |
| `unicode-bidi` 0.3.18, `unicode-linebreak` 0.1.5 | - | 2024 / 2023 | not checked | MIT OR Apache-2.0 / Apache-2.0 | Algorithms only. | crates.io |
| `rustybuzz` 0.20.1 (2024-11), `harfrust` 0.16.1 (2026-11-19) | - | - | alloc | MIT | Shaping engines; too heavy. | crates.io |
| `tjpgdec-rs` | 0.4.0 | 2026-01-09 | no_std option (`std` default) | MIT OR Apache-2.0 | TJpgDec port. README: workspace 3,100 / 3,500 / 9,644 bytes for the three speed levels. | https://github.com/planet0104/tjpgdec-rs |
| `tjpgd-rs` | 0.1.0 | 2026-07-11 | no_std; `alloc` optional; `embedded-io` input | MIT | Another TJpgDec translation. | https://github.com/decaday/tjpgd-rs |
| `trezor-tjpgdec` | 0.1.0 | - | no_std | **GPL-3.0-only** | Licence. | crates.io |
| `zune-jpeg` | 0.5.15 | 2026-03-26 | no_std **with alloc** ("one is required to link to a working allocator") | MIT OR Apache-2.0 OR Zlib | Baseline and progressive, but decodes the whole image into memory. Not viable for cover art. | https://github.com/etemesi254/zune-image |
| `jpeg-decoder` | 0.3.2 | 2025-06-21 | std | MIT OR Apache-2.0 | - | - |
| `minipng` | 1.0.0 | 2025-09-08 | no_std, no deps | 0BSD | Decodes into a caller buffer sized for the whole image. | crates.io |
| `png-decoder` 0.2.0, `embedded-png` 0.1.1, `zune-png` 0.5.2 | - | 2025 / 2026 | no_std + alloc (`miniz_oxide` 0.9.1) | permissive | Inflate needs a 32 KiB window plus row buffers. | crates.io |
| `tinybmp` | 0.7.0 | 2026-01-14 | no_std, no alloc | MIT OR Apache-2.0 | Streams from a byte slice. | embedded-graphics org |
| `tinygif` | 0.0.4 | 2023-07-07 | no_std | MIT/Apache-2.0 | Self-described "high memory footprint". | crates.io |
| `tinyqoi` 0.2.0, `tinytga` 0.5.0 | - | 2023 | no_std, no alloc | MIT OR Apache-2.0 | Good formats for UI assets converted at build time. | embedded-graphics org |
| `embedded-layout` | 0.4.2 | 2025-04-26 | no_std, no alloc | MIT | Alignment and linear layouts. | https://github.com/bugadani/embedded-layout |
| `embedded-menu` | 0.7.0 | 2026-08-07 | no_std, no alloc | MIT | Key-driven menus; fits a button UI. | https://github.com/bugadani/embedded-menu |
| `kolibri-embedded-gui` | 0.1.0 | 2025-02-25 | no_std, `heapless` | MIT OR Apache-2.0 | Immediate-mode, egui-like; supports partial redraw without a full framebuffer. Pointer-oriented. | crates.io |
| `buoyant` | 0.7.1 | 2026-10-10 | no_std | MIT OR Apache-2.0 | SwiftUI-like declarative views on embedded-graphics. | https://github.com/riley-williams/buoyant |
| `embedded-gui` | 0.2.7 | 2026-09-24 | no_std option | MIT OR Apache-2.0 | New, large (46k lines), f32-heavy. | crates.io |
| `slint` | 1.19.0 | 2026-11-16 | no_std + alloc on MCU | GPL-3.0-only OR royalty-free licence OR commercial | Software renderer can render line by line ("requires allocating at least enough RAM to store one single line of pixels") or into a full framebuffer. Needs a heap. | https://slint.dev |
| `mousefood` + `ratatui` | 0.5.3 / 0.30.2 | 2026-10-28 / 2026-06-19 | no_std + alloc | MIT OR Apache-2.0 / MIT | Character-cell UI on embedded-graphics; at 128x128 about 21x12 cells with a 6x10 font. | https://github.com/ratatui/mousefood |

### Could not verify

- RAM and flash cost of `slint`, `buoyant`, `mousefood` on a Cortex-M4 (no figures were found in the files read). Slint's often-quoted "under 300 KiB RAM" claim was not re-found today and is in any case the whole RAM of this device.
- Which CJK fonts `u8g2-fonts` exposes and their sizes; the licence of each font.
- Whether `lcd-async` requires a full framebuffer (my recollection is that it sends raw framebuffers rather than implementing `DrawTarget`; not re-checked).
- Whether `tjpgdec-rs`/`tjpgd-rs` are correct on arbitrary cover art (both are young ports).
- The SoC's hardware MJPEG decoder and "Scale" engine: listed in the datasheet block diagram, no register documentation seen.

## 8. USB device classes with embassy-usb

### Recommendation

- **Mass Storage now exists in embassy-usb, but only on `main`** (unreleased): `class::msc` implements Bulk-Only Transport + SCSI transparent commands for one LUN, over an `AsyncBlockDevice` trait with multi-block reads and writes. That, together with MTP, UAC2 and the new driver trait, is the main argument for building against a pinned `main` revision (section 1).
- **Virtual CD-ROM is not implemented in embassy-usb.** The in-tree MSC class hard-codes peripheral type "direct access", supports only LUN 0 and answers unknown opcodes with a sense error; there is no hook for extra commands. Two routes: (a) fork/extend the in-tree class (small: about five functions), or (b) use `snowscsi` 0.2.0, a new `no_std`, zero-alloc SCSI target library that already implements CD/DVD-ROM emulation (READ TOC, GET CONFIGURATION, READ DISC INFORMATION, and more) and is transport-agnostic, and write the BOT glue to embassy-usb endpoints. `snowscsi` is five weeks old with 41 downloads; treat it as a reference implementation as much as a dependency.
- **MTP on `main` is a transport, not a responder.** `class::mtp::device::MtpClass` handles the three endpoints, container framing, cancel/reset and the MS OS descriptor; `mtp/types.rs` supplies the operation codes and datasets such as `ObjectInfo`, but the object model (storage IDs, object handles, serving GetObjectInfo, SendObject and so on) is application code. MTP needs 3 endpoints.
- **Endpoint budget (4 FIFOs)** forces personalities: MSC (2) + HID keyboard (1) fits; MTP (3) + HID (1) fits; MSC + CDC-ACM (5) does not; UAC speaker with feedback (2) + HID (1) fits.
- **FIDO2/CTAP2 is not realistic on this device** as more than a demo; **TOTP/HOTP with HID keyboard typing is easy** (a few KB).

### Built-in classes: released versus `main`

| Class | In 0.6.0 (2026-03-20) | On `main` (2026-11-23) | Endpoints (non-control) | Notes |
|---|---|---|---|---|
| CDC-ACM | yes | yes (device + host) | 3 | `main`: `CdcAcmClass::new()` takes a `Config`; `line_coding()` returns `Option`. |
| CDC-NCM | yes | yes | 3 | Needs `embassy-net-driver-channel`. |
| CDC-ECM | no | yes | 3 | - |
| HID | yes (reports via the `usbd-hid` crate, default feature) | yes; built-in `KeyboardReport`, `MouseReport`, `ConsumerReport`, `GamepadReport`, composite presets; **`usbd-hid` feature removed** | 1 or 2 | Consumer-control reports give media keys. |
| MIDI | yes | yes | 2 | `main` validates max packet size for HS (512). |
| UAC1 | speaker only (`class::uac1`) | speaker and audio source (`class::uac::device`) | 1 or 2 | `main` requires `IsoEndpointIn` from the driver for IN streams and feedback. |
| UAC2 | no | `uac::device::uac2::io_box` (line out + line in, explicit feedback, FS or HS) | 2 to 4 | - |
| DFU | yes (`class::dfu`, used by `embassy-usb-dfu` 0.3.0) | yes | 0 | Tied to `embassy-boot` in `embassy-usb-dfu`. |
| WebUSB, MS OS descriptors | yes | yes; plus generic `class-vendor` bulk class with WinUSB/WebUSB | 2 (vendor) | Vendor bulk is a good 2-endpoint logging/RPC channel. |
| CMSIS-DAP v2 | yes | yes | 2 or 3 | - |
| **MSC** | **no** | **yes** (device + host), `SectorCache`, `BlockDeviceAdapter` for `block-device-driver` (feature `block-device-driver`) | 2 | See below. |
| **MTP** | **no** | **yes** (device, transport level) | 3 | See above. |
| CCID, BTH (USB HCI dongle), UVC, USBTMC, Printer, GUD | no | yes | - | Not relevant, except CCID for a smart-card style authenticator. |
| Host stack (`embassy_usb::host`, hub, VCP, GIP) | no (`embassy-usb-host` 0.1.0 separate, now deprecated) | yes | - | The SoC core is OTG-capable per the datasheet; host mode is a possible later feature. |

On `main` every class sits behind a `class-<name>` feature and `device`/`host` role features; default features enable everything, so use `default-features = false` to keep code size down.

### The in-tree MSC class (main)

```rust
pub trait AsyncBlockDevice {
    type Error;
    fn block_size(&self) -> u32;
    fn block_count(&self) -> u32;
    async fn read_blocks(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Self::Error>;   // multiple blocks per call
    async fn write_blocks(&mut self, lba: u32, data: &[u8]) -> Result<(), Self::Error>;
    async fn flush(&mut self) -> Result<(), Self::Error>;
    fn is_write_protected(&self) -> bool { false }
}
```

- Commands handled (from `msc/scsi.rs` and `msc/device/`): TEST UNIT READY, REQUEST SENSE, INQUIRY, READ/WRITE (6, 10, 12, 16), READ CAPACITY (10 and 16), READ FORMAT CAPACITIES, MODE SENSE (6/10), START STOP UNIT (eject), PREVENT ALLOW MEDIUM REMOVAL, SYNCHRONIZE CACHE(10), REPORT LUNS. `block_count` is `u32`, so 512-byte blocks cover 2 TiB.
- A synchronous `BlockDevice` trait with a blanket adapter also exists.
- The caller supplies the block buffer; its size sets the transfer chunk and therefore throughput. With code and data sharing 320 KB, a 4 to 16 KB transfer buffer is the realistic range.

### Third-party mass-storage and SCSI crates

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| embassy-usb `class::msc` | git main | unreleased | no_std, no alloc | MIT OR Apache-2.0 | Described above. | https://github.com/embassy-rs/embassy/tree/main/embassy-usb/src/class/msc |
| `snowscsi` | 0.2.0 | 2026-10-18 | no_std, zero-alloc, no unsafe (per README) | MIT OR Apache-2.0 | SCSI target core: block disk, read-only CD/DVD-ROM, DVD-RAM; "SPC-4/SBC-3 block commands plus the MMC-6 optical surface"; storage behind `SeekableStorage`/`FsStorage` traits; transports are non-blocking state machines (bare metal, Linux FunctionFS, iSCSI). Tested against the Linux initiator only, per README. | https://github.com/ImBearChild/SnowSCSI |
| `deluge-usb-classes` | 0.1.0 | 2026-11-06 | not checked | MIT OR Apache-2.0 | "Board-agnostic embassy-usb device classes (UAC2, USB-MIDI, MSC + Bulk-Only Transport)". Which embassy-usb version it targets was not checked. | https://github.com/FirestormAudio/deluge-sdk |
| `usbd-storage` | 3.0.0 | 2026-07-05 | no_std | MIT | For the `usb-device` stack, not embassy-usb. | https://github.com/apohrebniak/usbd-storage |
| `usbd_scsi`, `usbd_mass_storage`, `usbd_bulk_only_transport` | 0.1.0 | 2020-04-21 | no_std | - | `usb-device` based, dead since 2020. | crates.io |
| `ghostfat` | 0.5.0 | 2022-10-09 | no_std | - | Virtual FAT16 for UF2-style drag-and-drop. | crates.io |

No device-side MTP responder crate was found on crates.io (`mtp-rs`, `mtp`, `mtp_spec` are host/client side).

### Emulating a bootable USB stick or CD from an image file

- **USB stick (IMG):** nothing special on the device side. Present the image file's sectors through `AsyncBlockDevice` with 512-byte blocks; optionally report write-protected. Partition table and boot sectors are in the image. Keep the image file contiguous (or pre-resolve its cluster chain into an extent list) so LBA-to-card-sector mapping is O(1).
- **CD-ROM (ISO):** El Torito is on the disc; the device must look like an optical drive. The proven minimal set is what Linux `f_mass_storage` does in `cdrom` mode: INQUIRY peripheral device type 0x05 (ROM), read-only, 2048-byte blocks in READ CAPACITY, and two extra commands, **READ TOC (0x43)** and **READ HEADER (0x44)**, on top of the disk command set (INQUIRY, MODE SENSE 6/10, PREVENT ALLOW MEDIUM REMOVAL, READ 6/10/12/16, READ CAPACITY, READ FORMAT CAPACITIES, REQUEST SENSE, START STOP, SYNCHRONIZE CACHE, TEST UNIT READY, VERIFY). Source: `drivers/usb/gadget/function/f_mass_storage.c`. That is the mechanism DriveDroid-style tools rely on.
- **What firmware issues:** EDK2's `UsbMassBoot` uses only INQUIRY, REQUEST SENSE, MODE SENSE(6/10), READ CAPACITY, TEST UNIT READY, READ(10), WRITE(10), and takes the block size from READ CAPACITY; it special-cases peripheral types CD-ROM and optical. So UEFI needs nothing beyond type 0x05 and 2048-byte blocks. Windows and macOS optical stacks send more MMC commands (GET CONFIGURATION, GET EVENT STATUS NOTIFICATION, READ DISC INFORMATION, READ TOC formats beyond 0); answering them, or failing them with correct sense data, is where `snowscsi`'s command coverage helps.
- **The stock firmware already does this:** its RAM image contains the inquiry strings `ACTIONS USB DISK FOB 2.00` and `ACTIONS USB DISK CDROM  1.00` (public notes in section 10), so the controller is known to work as a CD-ROM device.
- Multi-LUN (disk + CD at once) is not supported by the in-tree class ("Only LUN 0 is supported").

### FIDO / CTAP2

| Name | Version | Date | Notes | URL |
|---|---|---|---|---|
| `trussed` | 0.2.1 | 2026-10-28 | Crypto service framework used by SoloKeys 2 and Nitrokey 3. Stores keys via `littlefs2`, a C binding (from memory, not re-checked today). | https://github.com/trussed-dev/trussed |
| `fido-authenticator` | 0.4.0 | 2026-11-20 | CTAP2 authenticator as a Trussed app. | https://github.com/trussed-dev/fido-authenticator |
| `ctap-types` | 0.6.0 | 2026-11-20 | `no_std` CTAP types. | trussed-dev |
| `usbd-ctaphid` | 0.5.0 | 2026-08-24 | CTAPHID for the **`usb-device`** stack. No embassy-usb port was found. | trussed-dev |
| `ctaphid-dispatch` | 0.4.0 | 2026-03-23 | Dispatch layer. | solokeys |
| `opensk` | 0.1.0 | 2026-10-22 | Google's OpenSK CTAP2 authenticator "as a library". Not inspected. | https://github.com/google/OpenSK |

Assessment: the Trussed stack targets MCUs with roughly this much RAM but with 0.5 to 1 MB of XIP flash, hardware key protection and a C filesystem dependency. Here the code would be paged from NOR, the key store would need a pure-Rust replacement, and, decisively, there is no secure element and the whole NOR is readable through the boot ROM's ADFU mode, so credentials could be extracted by anyone holding the device. A CTAPHID transport on embassy-usb's HID class is easy; a trustworthy authenticator is not achievable. The SoC does have AES, TRNG and a unique ID per the datasheet, which helps only marginally.

### TOTP / HOTP and typing codes

| Name | Version | Date | no_std | Licence | Notes |
|---|---|---|---|---|---|
| `hmac` | 0.13.0 | 2026-03-29 | yes | MIT OR Apache-2.0 | RustCrypto. MSRV 1.85. |
| `sha1` | 0.11.0 | 2026-03-27 | yes | MIT OR Apache-2.0 | RFC 6238 default hash. |
| `sha2` | 0.11.1 | 2026-10-12 | yes | MIT OR Apache-2.0 | For SHA-256/512 TOTP variants. |
| `totp-rfc` | 0.1.2 | 2026-07-30 | yes, alloc-free (per description) | MIT | New, 98 downloads. |
| `totp-lite` | 2.0.1 | 2023-10-30 | not checked | MIT | Thin layer over hmac/sha. |
| `totp-rs` | 6.0.0 | 2026-08-06 | std-oriented | MIT | Not for firmware. |
| `usbd-hid` | 0.10.2 | 2026-09-02 | yes | MIT OR Apache-2.0 | Report descriptors for embassy-usb 0.6.0's HID class; not needed on `main`. |

HOTP/TOTP is HMAC plus dynamic truncation, about 30 lines on top of `hmac` + `sha1`; writing it directly avoids a dependency on a three-month-old crate. TOTP needs correct wall-clock time: the RTC exists, but it must be set (over USB or from a file) and its drift matters over months. Typing the code is a HID boot keyboard sending digit key codes (1 endpoint). Secrets stored in NOR are extractable for the same ADFU reason as above; say so in the UI or encrypt them under a user PIN.

### Could not verify

- Whether any Windows/macOS host accepts the minimal READ TOC/READ HEADER CD-ROM set without GET CONFIGURATION; Linux gadget experience says mostly yes, but this was not tested or sourced today.
- `snowscsi`'s behaviour on real BIOS/UEFI/Windows hosts (its README cites Linux initiator tests only), and its RAM/code size.
- `deluge-usb-classes` contents and target embassy-usb version.
- Flash/RAM size of the Trussed + fido-authenticator stack (no figures were read today).
- Whether `totp-lite` is `no_std`.

## 9. Bluetooth

### Recommendation

- **First find out what the Bluetooth chip is.** Open the case or sniff the UART. The software plan differs by an order of magnitude between the two possibilities below, and public notes for a sibling platform point to the cheaper one.
  - *Autonomous Bluetooth audio SoC with a private UART command protocol* (documented for an ATJ2127 player with a Beken BK3266/BK3254): the module runs baseband, L2CAP, AVDTP/A2DP, AVRCP and the SBC encoder itself. The firmware needs only a framed command/event protocol driver and an audio feed to the module. No host stack.
  - *Standard HCI controller* (for example Realtek RTL8761B over H5): the firmware must contain a full Bluetooth Classic host. **No such host exists in pure Rust.**
- **There is no pure-Rust Bluetooth Classic (BR/EDR) host stack**, embedded or otherwise: no L2CAP BR/EDR, SDP server, RFCOMM, AVDTP/A2DP, AVCTP/AVRCP, HFP or HID-profile implementation was found on crates.io. Everything that mentions these profiles is a wrapper over an OS stack (BlueZ, CoreBluetooth, Windows) or over Google's Python Bumble.
- BLE is well served by `trouble-host`, but BLE does not carry A2DP audio. LE Audio (LC3 over isochronous channels) is not a substitute for talking to ordinary headphones today.
- If the chip is an HCI controller, the realistic options are: write a minimal A2DP-source host (large, see below), or drop Bluetooth audio from the first releases.

### Crates

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `trouble-host` | 0.8.0 | 2026-08-25 | no_std, no alloc | Apache-2.0 OR MIT | BLE host: central, peripheral, scanning, advertising, GATT client/server, L2CAP connection-oriented channels, security manager (pairing/bonding). An `iso.rs` module exists on `main`. **No BR/EDR**; no issue or PR about Classic or A2DP was found in a repository search. | https://github.com/embassy-rs/trouble |
| `bt-hci` | 0.11.0 | 2026-10-08 | no_std | MIT OR Apache-2.0 | HCI types and controller traits. Now includes Classic link-control commands: Inquiry, CreateConnection, Accept/RejectConnectionRequest, link-key and PIN replies, AuthenticationRequested, SetConnectionEncryption, RemoteNameRequest, Secure Simple Pairing replies, (Enhanced) SetupSynchronousConnection, plus `param/classic.rs` and synchronous (SCO) data packet types. MSRV 1.87. | https://github.com/embassy-rs/bt-hci |
| `bt-hci-transport` | 0.1.0 | 2026-08-03 | no_std | MIT OR Apache-2.0 | Transport trait, split out of `bt-hci`. | same |
| `bt-hci-serial` | 0.1.0 | 2026-08-18 | no_std | MIT OR Apache-2.0 | UART transport: **H4 only** ("UART transport layer protocol" with packet indicators) over `embedded-io`. **No H5 (three-wire) implementation**: a search of the whole workspace for "h5", "three-wire", "slip" finds nothing. | same |
| `bt-hci-usb` | 0.2.0 | 2026-08-12 | - | MIT OR Apache-2.0 | USB transport (host side). | same |
| `nrf-sdc` 0.4.0, `cyw43` 0.7.0 | - | 2026 | - | MIT OR Apache-2.0 | Controller bindings for Nordic and Infineon chips; no Realtek equivalent exists in Rust. | - |
| `bluetooth-hci` | 0.1.0 | 2020-05-08 | no_std | MIT/Apache-2.0 | Old BLE-oriented HCI types. Dead. | - |
| `rubble` | 0.0.4 | 2021-06-29 | no_std | 0BSD | BLE only, archived. | - |
| `burble` | 0.2.2 | 2023-03-22 | std | MPL-2.0 | BLE only, user-mode. | - |
| `bumble` | 0.2.0 | 2024-03-21 | std | Apache-2.0 | Rust API over Google's **Python** Bumble stack (which does implement Classic). Not embeddable. | https://github.com/google/bumble |
| `bluer` | 0.17.4 | 2025-06-06 | std, Linux | - | BlueZ D-Bus/socket interface. | - |
| `bluesdp` | 0.1.1 | 2026-07-15 | std | - | SDP *client* for Linux. Shows the SDP PDU format in Rust, nothing more. | crates.io |
| `obex-protocol` | 0.1.0 | 2026-10-30 | not checked | - | OBEX over RFCOMM/L2CAP provided by something else. | crates.io |
| `nanoaudio-sbc` | 0.1.1 | 2026-10-15 | no_std, no alloc | MIT | SBC encoder/decoder (section 3): the one A2DP building block that does exist. | nanoaudio |

### What a minimal A2DP source + AVRCP host would need (if the chip is an HCI controller)

Building blocks that exist: HCI command/event/ACL types (`bt-hci`), SBC encoder (`nanoaudio-sbc`), async UART plumbing (Embassy).

To be written:

1. H5 three-wire transport: SLIP framing, 4-byte header with header checksum, optional CRC16, sequence/ack numbers, sliding window (Linux uses a window of 4), link establishment (SYNC `01 7e`, CONFIG `03 fc`), retransmission timers, low-power wake messages.
2. Realtek bring-up: version read, patch selection, download, baud change (protocol below), and storing the patch + config blobs (tens of KB) on NOR or SD.
3. GAP for BR/EDR: inquiry, paging, role handling, Secure Simple Pairing ("just works"), link-key storage, reconnection.
4. L2CAP for BR/EDR: signalling channel, connection and configuration state machines, basic mode, segmentation and reassembly, flow control against the controller's ACL buffer credits.
5. SDP: a server with A2DP-source and AVRCP records, and a client to find the sink's AVDTP/AVRCP versions.
6. AVDTP: discover, get capabilities, set configuration, open, start, suspend, close, abort; media transport channel; RTP packetisation of SBC with timing.
7. A2DP SBC capability negotiation and bitpool selection.
8. AVCTP + AVRCP: at least pass-through commands as target (play/pause/next from the headphones) and absolute volume.

For scale, each of items 3 to 8 is a published specification of 50 to 400 pages, and C stacks that do this (BTstack, Zephyr's Classic host, Bluedroid) are tens of thousands of lines. I found no published RAM/flash footprint for a minimal A2DP source that I could cite today; plan on tens of KB of code and several KB of RAM, and treat any estimate as unverified. This is a multi-month sub-project with interoperability testing against many headphones.

### Bluetooth modules in Actions-based players

- **ATJ2127 (MIPS) player, community reverse-engineering notes** (`kilicdev/actions_flash`, `RE_NOTES.md`, repository created 2026-10-18): "Dual-Chip Architecture: Host ATJ2127 + Companion Beken BK3266/BK3254 Bluetooth Audio SoC over 1.5 MBaud DMA UART". The host applications are `BT.AP` (UI) and `BTENGINE.AP` (driver); the Beken chip contains "Bluetooth 4.2/5.0 stack, and SBC audio encoder/DSP"; "ATJ2127 does not encode A2DP or manage Bluetooth Baseband packets". Framing: `0xAA | SEQ | TYPE | OPCODE | payload | checksum` with checksum = two's-complement sum of bytes 1..n-2, 50 ms ACK timeout; opcodes include inquiry (0x01), cancel (0x02), remote name (0x03), connect (0x04), link state (0x05), version (0x06), A2DP start/suspend (0x16/0x17), AVRCP play/pause/stop/next/prev (0x20 to 0x25). Vendor config keys carry names such as `BTSTACK_OUTPUT_POWER_VALUE`, `BTMODULE_DEFAULT_BAUDRATE`, `BTSTACK_AVRCP_VOLUME_SYNC`. A controller firmware update file `BTUPG.HEX` is mentioned. How PCM audio reaches the module (I2S, analog, or over the UART) is not stated in the part read. https://github.com/kilicdev/actions_flash
- This is one unreviewed source about a different SoC generation, and parts of it read as inference rather than measurement. It establishes that the "UART-commanded audio SoC" architecture exists in this product family, not that this board uses it.
- **ATJ2157 players:** a patched AGPTEK A02 firmware changes Bluetooth reconnect retry counts by editing vendor configuration only (https://github.com/itamar050/agptek-a02-cfw), which shows the vendor firmware exposes Bluetooth behaviour as configuration but says nothing about the chip. The product page for the target device family lists "Bluetooth 5.0" for one model and "BT 4.1" for others (public notes in section 10), suggesting different modules across models.
- The ATJ2157 datasheet says the UART is "capable of speeds up to 6Mbps to enable connections with Bluetooth and other peripherals" and the SoC has two I2S transmitters, so both a UART-plus-I2S audio-SoC design and an HCI design are electrically plausible.
- Actions' own Bluetooth audio SoCs (ATS281x/ATS282x, SDK names US281B/US282A) are a separate product line (speakers, headsets); a community notes repository exists for ATS2819 (https://github.com/easmith/ats2819-notes). No evidence was found of an ATS28xx used as the module inside an ATJ2157 player.
- Searches for the specific terms "US215A 蓝牙发射", "ATJ2157 蓝牙", "RTL8761ATV MP3" were started by a helper that did not finish; I have no results to report for them.

### Realtek H5 patch download protocol (RTL8761B family, UART)

Sources: Linux `drivers/bluetooth/hci_h5.c` and `drivers/bluetooth/btrtl.c` (kernel.org), and Realtek's `rtk_hciattach` (`hciattach_rtk.c`). All statements below were checked against those files except where marked.

1. Open the UART at 115200 baud (`h5_btrtl_open` sets 115200). Realtek H5 modules normally use even parity and hardware flow control; that detail is from memory, check the module.
2. Establish the H5 link: send SYNC (`01 7e`), answer SYNC RESPONSE, exchange CONFIG (`03 fc` + config field) until active. Linux uses a transmit window of at most 4 and a 100 ms sync timeout.
3. Send HCI Read Local Version; match `lmp_subver`, `hci_rev`, `hci_ver` and bus type against a table. The UART entry `IC_INFO(RTL_ROM_LMP_8761A, 0xb, 0xa, HCI_UART)` selects firmware `rtl_bt/rtl8761b_fw` and config `rtl_bt/rtl8761b_config` (hardware name "rtl8761btv"); both files are in linux-firmware.
4. Send vendor command **0xFC6D** to read the ROM version.
5. Parse the firmware file: "Realtech" signature (v1 epatch; v2 uses "RTBTCore"), a table of patches keyed by chip id; choose the patch for ROM version + 1; the config file (magic `0x8723ab55`, list of offset/length/data entries) is appended to the patch image.
6. For UART, read the baud-rate entry from the config file (`btrtl_get_uart_settings`), send vendor command **0xFC17** with the 4-byte Realtek baud-rate code, wait briefly, then switch the host UART to the new rate.
7. Download with vendor command **0xFC20** in fragments of at most 252 bytes (`RTL_FRAG_LEN`), each prefixed by an index byte that increments, wraps from 0x7f back to 1, and has bit 7 set on the last fragment. Each fragment is acknowledged with a Command Complete.
8. Read Local Version again to confirm the patched firmware is running, then proceed with normal HCI initialisation.

No Rust implementation of this exists (crates.io search for Realtek Bluetooth crates returned nothing relevant; `bt-hci` has no Realtek vendor module).

### Could not verify

- The Bluetooth chip on the target board, and how audio is passed to it.
- Whether the Beken-style protocol in the ATJ2127 notes is also what ATJ2157 firmware (US215A) uses.
- Published footprints for minimal A2DP-source stacks.
- Whether Realtek RTL8762/RTL8763 (which are application SoCs rather than plain controllers) use the same download protocol; only the RTL8761 family was checked.
- `trouble-host` RAM/flash figures (its `docs/` and `benchmarks/` directories exist but were not read).

## 10. The SoC itself

### Most decision-relevant facts

- **An official datasheet is public: "ATJ2157 Datasheet", Latest Version V1.2, dated 2022-04-30, 336 pages.** Listing page: https://datasheet4u.com/datasheets/Actions/ATJ2157/1563425 (the PDF was fetched by a helper; I read its text conversion, not the PDF layout). It contains register-level chapters for PMU, OSC, CMU (clocks), RMU (resets), RTC and the timer/watchdog block, DMA (10 channels), SPI0 and SPI1, UART, TWI, DAC, ADC, I2S, LCD controller, PWM, SARADC/LRADC, and GPIO/MFP, plus the interrupt numbering and the memory map. **It has only feature lists, no registers, for NAND, SD/MMC and USB**, and no chapter at all for the SPI cache controller, AES, TRNG, MJPEG or the FFT/FIR/IIR engine.
- **Maximum core frequency:** "Up to 288MHz M4F processor Core" (datasheet 1.2). Community notes: core PLL = multiplier x 6 MHz; the boot ROM's recovery mode leaves the CPU at 48 MHz from the device PLL; one community firmware runs at 96 MHz.
- **Memory per datasheet:** "Built-in 224KB RAM and 64KB ROM", "Built-in 32KB Icache and 32KB Dcache". Memory map: ROM `0x00000000` (64 KB), RAM0 to RAM6 at `0x00100000` to `0x00137FFF` (7 x 32 KB), "HW RAM" 32 KB at `0x00150000`, and the heading "SRAM Mapping (total:384K)". This differs from the measured 320 KB in four blocks (224 KB at `0x100000`, 32 KB at `0x158000`, 32 KB at `0x200000`, 32 KB at `0x300000`) recorded in the public port notes.
- **Hardware XIP lead.** Datasheet 3.3 "CACHE": "Support code runs directly in SPI nor/SRAM", "Caching data for code and read-only data in SPI Nor", "32KB Icache of SPI Nor, with 4-ways associative and 8k bytes cache size of a way", "32KB Dcache of SPI SRAM", "Support performance profile". The memory map lists "SPI Cache" at `0xC0050000` and "SPI1 Cache" at `0xC0060000`; CMU and RMU have `SPI0CACHECLKEN`/`SPI0CACHERESET` bits; the SPI0 chapter says "SPI is a combination module which includes conventional SPI and SPI_Cache interface", with CRC16 and a randomizer on the cache path, a `CACHE_ERROR_ADDR` register and "cache abort". The cache controller's own registers and the address window through which cached NOR appears are **not documented**. Hypothesis worth a day of probing: the two 32 KB blocks measured at `0x200000` and `0x300000` (each "aliased over 1 MB", with separate DMA addresses) are the I-cache and D-cache memories, and enabling the SPI0 cache controller would give memory-mapped, demand-fetched NOR without an MPU pager. The stock firmware uses banked code (strings `ap_bank`, `kernel_bank`); whether it also uses the cache path is unknown. Unverified in every respect.
- **No DSP core.** One Cortex-M4F with single-precision FPU and MPU. Accelerators listed: AES, TRNG, unique ID, "FFT/FIR/IIR hardware engine", "Integrated Motion JPEG Video Decoder, up to 30fps@QVGA", and a "Scale" block in the diagram. None has public register documentation.
- **FM is not on-chip.** The ADC has "Two MIC/AUXIN/FM Analog ADC channels"; the tuner is an external I2C chip whose audio goes into the ADC.
- **I2S:** two transmitters and two receivers, master or slave, 8 kHz to 192 kHz; registers documented (chapter 8.3). Also PDM in/out and up to 4 digital microphones.
- **Audio codec:** stereo sigma-delta DAC (SNR above 98 dB), 18 mW headphone amplifier, sample rates 8 to 96 kHz in both 44.1 and 48 kHz families, 24-bit FIFOs; three ADC channels.
- **USB:** "Support USB2.0 HS, Compatible with EHCI"; OTG with SRP/HNP; "Hardware configurable endpoints (four data endpoint and one pair controller endpoint) as HS/FS device"; "embedded DMA access to FIFO"; "PVCI-compatible system bus". The wording matches a Faraday dual-role core. Base `0xC0140000`, IRQ 12.
- **Interrupts:** up to 64, four priority levels (so 2 priority bits: `prio-bits-2` for `embassy-hal-internal`). Table (datasheet 3.2.1): 1 WD_2Hz, 2 to 5 TIMER0..3, 6 RTC, 7/8 I2SRX0/1, 10/11 I2STX0/1, 12 USB, 13 to 15 SPI0..2, 16 NAND, 17 SDC, 18 SDIO, 19/20 TWI0/1, 21 to 24 UART0..3, 26 to 35 DMA0..9, 36 EXTIRQ, 39 DAC, 40 ADC01, 41 ADC23, 42 LCD, 48 MPU, 50 SDMA.
- **Peripheral bases** (datasheet 3.4): RMU/CMU `0xC0000000`, PMU `0xC0010000`, Timer/RTC/WD `0xC0030000`, memory controller `0xC0040000`, DMA `0xC0070000`, SPI0/1/2 `0xC00A0000`/`B`/`C`, UART0..3 `0xC00D0000`..`0xC0100000`, TWI0/1 `0xC0110000`/`0xC0120000`, USB `0xC0140000`, NAND `0xC0150000`, SD/MMC `0xC0160000`, SDIO `0xC0170000`, audio `0xC0180000`, PWM `0xC0190000`, LCD `0xC01A0000`, GPIO `0xC01C0000`.
- **Debug pins:** the datasheet text contains no SWD/JTAG pin naming that I could find. The A02-OS hardware notes list "SWD | GPIO62/63" for the AGPTEK A02. Unverified for other boards and worth a probe.
- **Vendor software:** uC/OS-II based SDK. For the MIPS ATJ2127 the SDK is called US212A and has leaked in full; for the ATJ2157 the community refers to "US215A" (identified from module naming and strings; no leaked source was found).

### Sources

| What | URL | Contains | Date |
|---|---|---|---|
| ATJ2157 datasheet V1.2 | https://datasheet4u.com/datasheets/Actions/ATJ2157/1563425 | See above. 336 pages. | 2022-04-30 |
| `ilyakurdyukov/actions_flash` | https://github.com/ilyakurdyukov/actions_flash | ADFU firmware dumper for ATJ2127/ATJ2157 on Linux; `payload_arm/` has RAM payload examples for the ARM part (start code, linker script, ADFU server, NAND read). | created 2025-05-04, pushed 2026-11-22 |
| `TheWirelessPhoenix/a02-os` | https://github.com/TheWirelessPhoenix/a02-os | **A complete bare-metal C replacement OS for an ATJ2157 player** (AGPTEK A02): MP3/WAV from microSD (FatFs + minimp3), recorder, themes. `docs/HARDWARE.md` documents by reverse engineering: pin map, PLL and CMU usage, the **SD controller register layout** (`+0x04` command control, `+0x08` status, `+0x0C` command, `+0x10` argument, `+0x14..0x20` response, `+0x28` FIFO, `+0x2C` block size, `+0x30` block count; DMA channel 4, config `0x85` read / `0x8500` write; CMD12/CMD13 handling), LCD controller FIFO protocol, DAC and PA registers, key ADC levels, watchdog. MIT. | created 2026-09-27 |
| `zanzipanzi/atj2157-player-toolkit` | https://github.com/zanzipanzi/atj2157-player-toolkit | RE notes and tools for ATJ2157 "nano-clone" players: ADFU, SPI flash scrambler, firmware and font formats, SPI ROM routine headers (`payload/spi_rom.h`). MIT. | created 2026-10-07, pushed 2026-11-01 |
| `timschmidt/yoton-rockbox` | https://github.com/timschmidt/yoton-rockbox | Rockbox port to the Yoton YM03 (ATJ2157) as a patch series plus `FINDINGS.md`: measured memory map, NOR layout, boot ROM flow, board pinout, and RAM payloads including `mpupage.c`. | created 2026-11-23 |
| `kilicdev/actions_flash` | https://github.com/kilicdev/actions_flash | Fork with `RE_NOTES.md` on the ATJ2127 overlay loader, syscall ABI, some registers and the Bluetooth UART protocol (section 9). Single-author notes, not cross-checked. | created 2026-10-18 |
| `Suber/PD196_ATJ2127` | https://github.com/Suber/PD196_ATJ2127 | **Leaked US212A SDK for ATJ2127** (`psp_rel/`, `case/fwpkg/`): uC/OS headers, `psp_rel/include/asm-mips/mach-gl5110/GL5110_Design_Specification.h` (374 KB auto-generated register definitions, "GL5110 Spec Version_V1.09", 2011-12-30), `dma.h`, `irq.h`, `addr_map.h`, `link_memory_map.h`, firmware packaging config. No licence. | 2013-08-02 |
| `malos17713/US212A_ATJ2127` | https://github.com/malos17713/US212A_ATJ2127 | Another copy of the US212A SDK including `Doc/US212A 应用程序设计指南.pdf` (8.7 MB application design guide) and interface manuals (`.chm`) for PSP, common, key, LCD and UI drivers; `case/fwpkg/font/UNICODE.FON`. | pushed 2018-03-30 |
| `nfd/atj2127decrypt` | https://github.com/nfd/atj2127decrypt | Decrypts ATJ2127 `UPGRADE.HEX`. MIT. | pushed 2026-02-04 |
| Rockbox `utils/atj2137` | https://github.com/Rockbox/rockbox/tree/master/utils/atj2137 | `atjboottool` (firmware container decryption, also used for ATJ2157 `.fw` files per community notes), `adfuload`, MIPS test binaries. For ATJ213x. | - |
| Rockbox forum thread 51281 | https://forums.rockbox.org/index.php?topic=51281.0 | "Firmware support for ruizu x02" (ATJ2127): concluded RAM is too small for Rockbox; links to datasheets and the SDK. | 2016 onwards |
| "More ATJ21XX stuff" (Tom Tildavaan) | https://infosec.press/tildavaan/more-atj21xx-stuff | Overview of ATJ212x (MIPS) versus ATJ215x (Cortex-M4F, "288MHz", "224KB RAM"), uC/OS-II SDK, where `ADFUS.BIN` lives inside firmware images. States "Apart from data sheets and pinouts, we have found nothing for the ARM variant". | 2026-03-08 |
| `itamar050/agptek-a02-cfw`, `phanfeld/odejoi-x91-firmware` | github.com/itamar050/agptek-a02-cfw, github.com/phanfeld/odejoi-x91-firmware | Patched vendor firmware for ATJ2157 players. The first documents the update container: RC4 keyed through an ECIES handshake on sect233k1, decrypting to an SQLite database (`FileTable`) of about 180 modules and resources. | 2026-10 / 2026-11 |
| `vichj99/mechen-m30-rockbox`, `averagenative/mechen-m30-firmware`, `catskull/clip-jam-fw` | github.com (same names) | ATJ2127 (MIPS) work; the first states "Actions GL5110 (die name ATJ2127)" and derives register maps from a GL5110 datasheet. | 2026-08 to 2026-11 |
| `easmith/ats2819-notes` | https://github.com/easmith/ats2819-notes | Notes for the ATS2819 / US281B Bluetooth audio platform. | 2026-10-25 |
| `BlackLikeaCat/actions-adfu-tool` | https://github.com/BlackLikeaCat/actions-adfu-tool | Windows tool to read boot ROM and memory over ADFU across many Actions parts. GPL-2.0. | 2026-11 |

### Details

- **Internal codenames.** GL5110 = ATJ2127 (stated by `vichj99/mechen-m30-rockbox`; the leaked header is under `mach-gl5110`). The header name asked about, `gl5115_register.h`, was not found in any repository examined. No GL51xx codename for the ATJ2157 was found. The public port notes say the ATJ2157 "shares peripheral IP blocks with ATJ2127 at different addresses", so `GL5110_Design_Specification.h` is a useful bitfield reference for blocks missing from the ATJ2157 datasheet (SD, USB, NAND) once offsets are confirmed on hardware. It carries no licence; use it as documentation, not as copied code.
- **Register-map coverage for the peripherals this project needs:**

| Peripheral | Datasheet V1.2 | Other public source |
|---|---|---|
| CMU/PLL, RMU, PMU, RTC, timers, watchdog | yes | A02-OS notes |
| DMA | yes (chapter 5) | A02-OS (channel 4 config values), yoton-rockbox notes (DMA address aliases) |
| UART, TWI (I2C), SPI0/SPI1 | yes | toolkit `spi_rom.h` for ROM routines |
| DAC, ADC, I2S | yes (chapter 8) | A02-OS power-up sequence |
| LCD controller (8080), PWM (backlight) | yes (chapter 9) | A02-OS command/data FIFO protocol; panel init table from stock firmware |
| LRADC keys, GPIO/MFP | yes | A02-OS pin map and key levels |
| SD/MMC host | **no** (features only) | A02-OS register layout; GL5110 header by analogy |
| USB device controller | **no** (features only) | none found for ATJ2157; GL5110 header and Linux `fotg210-udc` by analogy |
| SPI cache controller, AES, TRNG, FFT/FIR/IIR, MJPEG | **no** | none |

- **Stock firmware structure** (from public notes): mask ROM, boot blocks on NOR (512-byte `mbrec` loaded to `0x101000`), LFI firmware image with a directory, kernel segments, then `kernel.drv`, `manager.ap`, applications `*.ap`, drivers `*.drv` (including `fat16s.drv`, `fat32n.drv`, `exfatn.drv`, `exfats.drv`, `card.drv`, `uhost.drv`, `nor_ud.drv`), resources `*.sty`. Code is banked ("ap_bank", "kernel_bank"), i.e. the vendor solves the small-RAM problem with overlays, not XIP. The NOR content is scrambled by a hardware randomizer in the SPI controller.
- **Rockbox upstream:** the Rockbox tree contains only `utils/atj2137` tooling; there is no ATJ2157 or ATJ2127 target. `gerrit.rockbox.org` and `forums.rockbox.org` put up a bot challenge to automated fetches, so Gerrit could not be searched for pending changes. Outside upstream, 2026 has seen a burst of ATJ2157 work: A02-OS (September), the player toolkit (October), firmware patch projects (October, November), and the yoton-rockbox port (pushed 2026-11-23) which already implements an MPU demand pager in C.

### Could not verify

- The direct PDF download URL of the datasheet (only the listing page URL is recorded) and whether newer datasheet revisions exist.
- Whether the SPI NOR cache/XIP path is functional; its register interface; its address window.
- The reason the datasheet's RAM map (224 KB + 32 KB "HW RAM", "total 384K") differs from the measured 320 KB.
- SWD pin assignment on boards other than the A02.
- Any leaked US215A (ATJ2157) SDK source or register header: none found on GitHub. Gitee, CSDN and 4pda were on the helper's list but I have no results from them.
- Rockbox Gerrit activity for ATJ2157 (blocked by a bot challenge).
- The search terms "US215A", "ATS2819/ATS2825" in relation to MP3-player Bluetooth modules (not completed).

## 11. Debug and logging

### Recommendation

- **Use `defmt` with a ring-buffer global logger that survives reset: `defmt-persist` 0.2.1.** It is a `defmt::global_logger` writing into a RAM region reserved in the linker script outside `.bss`/`.data`, and hands you a `Consumer` to drain over any transport. After a panic and reset, the log leading up to the panic is still there and is sent on the next boot. It also stores a firmware identifier (the README recommends the GNU build ID) so the host knows which ELF decodes the recovered frames. On this device the reserved region must also be outside whatever the boot ROM and loader overwrite (the ROM uses `0x100000` to `0x101B18` during boot per public notes).
- **Transport:** drain the consumer to a UART when one is found (the datasheet has four UARTs; debug-print strings in the stock firmware suggest a console exists), and otherwise to USB. Because of the 4-FIFO limit, prefer a **vendor bulk IN endpoint** (1 FIFO) or a vendor control request over CDC-ACM (3 FIFOs) when Mass Storage is active.
- **Host side:** `defmt-print` 1.2.0 decodes from stdin, a TCP socket, or directly from a serial port (`defmt-print -e firmware.elf serial`, with port and baud options or the `SERIAL_PORT` / `SERIAL_BAUD` environment variables); pipe a USB reader into stdin for the vendor endpoint.
- **Why defmt rather than `log` here:** format strings stay in the ELF, not in the image, and formatting happens on the host. With code and rodata paged from NOR in 1 KB lines, removing `core::fmt` machinery and strings from the image matters more than usual. No published size comparison was found; the claim is structural.
- **SWD is worth one probing session** before committing to probe-less development: a sibling project's notes list SWD on GPIO62/63 for the same SoC (section 10). With SWD, `defmt-rtt` and `probe-rs` work unchanged since everything is in RAM.

### Crates

| Name | Version | Date | no_std | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `defmt` | 1.2.0 | 2026-10-15 | yes | MIT OR Apache-2.0 | 1.2.0 adds `PanicInfo::message` support for static strings and bumps embedded MSRV to 1.81. Unreleased work targets code size (enum encoding, inline-asm string index loads on ARMv7+). | https://github.com/knurling-rs/defmt |
| `defmt-persist` | 0.2.1 | 2026-10-22 | yes | MIT OR Apache-2.0 | Persistent ring-buffer logger (described as the combination of `panic-persist` and `defmt-brtt`). Symbols `__defmt_persist_start/_end`. | https://github.com/korken89/defmt-persist |
| `defmt-serial` | 0.14.0 | 2026-07-22 | yes | MIT | Global logger writing synchronously to any `embedded_io::Write` inside a critical section. Simple; blocks while writing. | https://github.com/gauteh/defmt-serial |
| `defmt-brtt` | 0.1.2 | 2026-08-13 | yes | MIT OR Apache-2.0 | RTT and/or BBQueue sink. | https://github.com/datdenkikniet/defmt-brtt |
| `defmt-bbq` | 0.1.0 | 2021-12-17 | yes | MIT OR Apache-2.0 | Original bbqueue-based logger. Old (depends on the bbqueue of that era; `bbqueue` is now 0.7.0). | https://github.com/jamesmunns/defmt-bbq |
| `defmt-embassy-usbserial` | 0.2.1 | 2025-10-27 | yes | MIT OR MPL-2.0 | defmt over embassy-usb CDC-ACM. Low usage. | https://github.com/robjwells/defmt-embassy-usbserial |
| `embassy-defmt-usb` | 0.0.2 | 2026-07-15 | yes | MIT | "Generic defmt USB CDC-ACM drain transport for embassy". Very low usage. | https://github.com/sunipkm/embassy-defmt-usb |
| `embassy-usb-logger` | 0.6.0 | 2026-03-20 | yes | MIT OR Apache-2.0 | **`log` facade, not defmt**: text over CDC-ACM through an `embassy_sync::Pipe` (`run!(1024, LevelFilter::Info, driver)`). | embassy repo |
| `defmt-rtt` | 1.3.1 | 2026-10-15 | yes | MIT OR Apache-2.0 | Needs a debug probe. | defmt repo |
| `defmt-print` / `defmt-decoder` | 1.2.0 | 2026-10-15 | host | MIT OR Apache-2.0 | Subcommands `stdin` (default), `tcp`, `serial` (port, baud, DTR). | defmt repo |
| `defmt-or-log` | 0.2.3 | 2025-10-29 | yes | MIT OR Apache-2.0 | Macros that forward to defmt or `log`, so library crates build on the host with `log`. Useful for section 13. | https://github.com/t-moe/defmt-or-log |
| `panic-persist` | 0.3.0 | 2021-09-17 | yes | MIT OR Apache-2.0 | Writes the panic message text to a reserved RAM region and resets; read it on next boot. Uses `core::fmt` (size). Stable but old. | https://github.com/jamesmunns/panic-persist |
| `panic-probe` 1.0.0, `panic-halt` 1.0.0, `panic-reset` 0.1.1 | - | 2025 / 2024 / 2022 | yes | MIT OR Apache-2.0 | Standard handlers. | - |
| `postcard-rpc` | 0.12.1 | 2025-12-04 | yes | MIT OR Apache-2.0 | If a structured host/device command channel over the vendor endpoint is wanted in addition to logs. | https://github.com/jamesmunns/postcard-rpc |

### Could not verify

- Measured flash/RAM cost of defmt versus `log` on this kind of image.
- Whether `defmt-persist`'s consumer API is async-friendly without an extra wake mechanism (README shows a polling drain loop).
- Location of a UART console or SWD pads on the target board.

## 12. Storage for settings

### Recommendation

- **`sequential-storage` 8.0.2** for settings in a few 4 KB NOR sectors: key-value map and FIFO queue, wear-levelled over the pages cyclically, power-fail safe, CRC-protected, async, built on `embedded-storage-async`, zero RAM without a cache and optional small caches. Its README states production use "powering millions of devices". There is free NOR on this board (public notes: `0x770000` to `0x7DFFFF`, 448 KB, erased).
- `ekv` 1.0.0 is the alternative for large key counts (LSM tree, transactions); its own README says linear-scan stores such as `sequential-storage` are faster below roughly 1,000 keys, and a transaction always opens a new file (at least one page erase), which is a poor fit for frequent single-setting writes. Its on-disk format is declared not stable and it does not use `embedded-storage` traits.
- Write a NOR driver implementing `embedded-storage-async::nor_flash::NorFlash` (`READ_SIZE`, `WRITE_SIZE`, `ERASE_SIZE = 4096`) once; the pager, the settings store and any firmware-update code can share it through `embassy-embedded-hal`'s `Partition`. Note that the stock NOR image is hardware-scrambled; decide whether the settings area is accessed with the scrambler on or off and keep it consistent.
- Playback position, play counts and the media database belong on the SD card as files, not in NOR.

### Crates

| Name | Version | Date | no_std / alloc | Licence | Notes | URL |
|---|---|---|---|---|---|---|
| `sequential-storage` | 8.0.2 | 2026-10-01 | no_std, no alloc | MIT OR Apache-2.0 | API changed twice recently: 7.0.0 (2025-12-16) moved to `MapStorage`/`QueueStorage` structs that own the flash; 8.0.0 (2026-07-08) restructured caches around one `Cache` type and is "disk-compatible with 7.0". Optional `postcard` value support (7.1.0). Cache RAM: 0, 24 bytes, or per-page/per-key arrays. Cancelling a write future can corrupt state that is then repaired. MSRV 1.87. | https://github.com/tweedegolf/sequential-storage |
| `ekv` | 1.0.0 | 2024-11-12 | no_std | MIT OR Apache-2.0 | O(log n), transactions, cursors, wear levelling. No release since 2024-11. | https://github.com/embassy-rs/ekv |
| `tickv` | 2.0.0 | 2025-01-14 | no_std | MIT OR Apache-2.0 | Tock's hash-based store. Low use outside Tock. | https://github.com/tock/tock |
| `embedded-storage` / `-async` | 0.3.2 / 0.4.2 | 2026-09-20 | no_std | MIT OR Apache-2.0 | First releases since 2023-12. MSRV 1.87. | https://github.com/rust-embedded-community/embedded-storage |
| `littlefs2` | 0.8.1 | 2026-08-08 | C binding | Apache-2.0 OR MIT | Excluded (C). | - |

### Could not verify

- `sequential-storage` code size on thumbv7em.
- Other new stores seen in search (`horton`, `simple-kvdb`, `wasefire-store`) were not examined.

## 13. Testing on the host

### Recommendation

- **Split the workspace so that almost everything is a `no_std` library crate with no chip dependency**: `app-core` (state machines, playlist, settings model), `ui` (draws to any `DrawTarget<Color = Rgb565>`), `codecs`, `vfs`, `usb-classes`, with the hardware behind small traits (`embedded-hal-async`, `embedded-io-async`, `block-device-driver`, `embedded-storage-async`). Only a thin `firmware` crate depends on the PAC and `cortex-m-rt`.
- **UI on the desktop:** `embedded-graphics-simulator` 0.8.0. `SimulatorDisplay<Rgb565>` is a `DrawTarget`; with `default-features = false` it needs no SDL2 and can still export images (`to_rgb_output_image`), which is the basis for **screenshot tests**: render a screen, compare to a reference PNG (or use `insta` for snapshot management). With SDL enabled, `EG_SIMULATOR_DUMP=shot.png` and `EG_SIMULATOR_CHECK=shot.png` give dump/compare without writing code.
- **Embassy on the host:** `embassy-executor` with `platform-std` + `executor-thread` (the `arch-std` name is gone since 0.10.0; `Executor::run_until` exists since 0.10.0) and `embassy-time` with the `std` feature gives real timers. For deterministic tests use `embassy-time`'s `mock-driver` feature: `MockDriver::get().advance(Duration)` moves time by hand and fires due alarms, and `reset()` clears state between tests. `std` and `mock-driver` are alternative time drivers; pick one per test binary.
- **Storage and filesystems:** implement `block-device-driver::BlockDevice` over a file to run the real FAT/exFAT code against card images; `embedded-storage-inmemory` or a 30-line RAM `NorFlash` for the settings store. Test power-fail by truncating writes.
- **Decoders:** run them on the host against reference vectors and compare with ffmpeg output; this is where the young codec crates from section 3 should be vetted.
- **Logging in shared crates:** `defmt-or-log` (or feature-gated macros) so library crates log through `log` on the host and `defmt` on the device.
- **On-device tests** without a probe are awkward; `embedded-test` and `defmt-test` assume probe-rs. If SWD turns out to be available they become usable. Otherwise a tiny test runner over the USB vendor endpoint is the fallback.

### Crates

| Name | Version | Date | Licence | Notes | URL |
|---|---|---|---|---|---|
| `embedded-graphics-simulator` | 0.8.0 | 2025-10-10 | MIT OR Apache-2.0 | SDL2 window (default feature `with-sdl`), headless image export, env-var dump/check. | https://github.com/embedded-graphics/simulator |
| `embassy-executor` (`platform-std`) | 0.10.0 | 2026-03-20 | MIT OR Apache-2.0 | Runs the same `#[embassy_executor::task]` code on std threads. | embassy repo |
| `embassy-time` (`std`, `mock-driver`) | 0.5.1 | 2026-03-20 | MIT OR Apache-2.0 | `main` fixes a deadlock in the std driver between `schedule_wake()` and the alarm thread (unreleased). | embassy repo |
| `embedded-hal-mock` | 0.11.1 | 2024-06-02 | MIT OR Apache-2.0 | Expectation-based mocks for I2C/SPI/GPIO/delay, including embedded-hal 1.0 and async. Good for the FM tuner and accelerometer drivers. | https://github.com/dbrgn/embedded-hal-mock |
| `embedded-storage-inmemory` | 0.1.1 | 2023-12-12 | MIT OR Apache-2.0 | RAM-backed `embedded-storage`. | https://github.com/lulf/embedded-storage-inmemory |
| `insta` | 1.49.0 | 2026-10-03 | Apache-2.0 | Snapshot testing; binary snapshots can hold PNG screenshots. | https://github.com/mitsuhiko/insta |
| `embedded-io-adapters` | 0.7.0 | 2025-09-30 | MIT OR Apache-2.0 | Wrap `std::io`/tokio types as `embedded-io(-async)`. | embedded-hal repo |
| `critical-section` (`std` feature) | 1.2.0 | 2024-10-16 | MIT OR Apache-2.0 | Needed by `embassy-sync` on the host. | - |
| `embedded-test` | 0.7.3 | 2026-10-27 | MIT OR Apache-2.0 | On-device test harness for probe-rs. | https://github.com/probe-rs/embedded-test |
| `embassy-mock` | 0.5.0 | 2025-06-15 | MIT | Unofficial mocks for Embassy types. Low use. | https://github.com/cdunster/embassy-mock |

### Could not verify

- That `embedded-graphics-simulator` 0.8.0 is compatible with `embedded-graphics` 0.8.2 without a version pin (expected, same 0.8 line).
- Any emulator for the SoC (QEMU has no ATJ2157 machine; a generic Cortex-M4 machine would only exercise core code, and nothing in this area was researched).

