# 03 — HAL and board drivers

What to build in `atj2157-pac`, `atj2157-hal` and `yoton-bsp`, driver by
driver: where the facts are (the datasheet in `docs/vendor/` first for every
block it covers — 01 §1 — then the reports and the Rockbox port), the shape of the API, design points that are not
obvious, the bring-up test, and the acceptance check. Unknowns are referenced
by their ids in [01-hardware.md](01-hardware.md) §9.

## 1. Conventions

* **PAC** (`atj2157-pac`): hand-written, one module per block, a thin typed
  register layer (read/write/modify with field accessors). Generate it from a
  small YAML/SVD description with `chiptool` or write it by hand — whichever
  the worker can keep readable (docs/research/rust-ecosystem.md §2); the
  description file is then the single source and carries trust tags
  (`[HW]`, `[code]`, `[inferred]`) per field. Unknown bits are named
  `_unk_b12` rather than invented.
* **HAL** (`atj2157-hal`): owns clock gates and resets through a `Clocks`
  token; drivers take pin and DMA-channel ownership by type; every driver is
  `async` where it waits, has a `Stats` struct, and never busy-waits longer
  than 50 µs outside initialisation.
* **BSP** (`yoton-bsp`): the pin table of 01 §5, device instances, the key
  ladders, panel init table, tuner, accelerometer, Bluetooth module, and the
  power policy numbers (battery thresholds, charger values).
* **Bring-up images** (`bringup/<name>`): each driver first lives in a
  RAM-only image under 96 KiB that runs under `xtask dev ram … --test`,
  writes a result block and a picture on the screen where that helps, and
  returns to ADFU. They stay in the tree as regression tests (`hil drivers`).
* Sequences copied from the vendor without understanding are marked
  `VERBATIM` (02 §10 rule 8).

## 2. Clocks and performance levels

Facts: system.md §1 (sources, `CMU_SYSCLK`, level table, switching order,
peripheral clock derivations).

### 2.1 Levels

| Level | Core clock | Source setting | Voltage code | Availability |
|---|---|---|---|---|
| `Idle` | 6 MHz | HOSC ÷ 4, COREPLL off | `0x0C` | after U-PWR-4 |
| `Low` | 24 MHz | HOSC, COREPLL off | `0x0C` | first |
| `Base` | 48 MHz | COREPLL N = 8 | `0x0C` | first (the ROM leaves 48 MHz at the 1.2 V default, which is what the port ran [HW]; 48 MHz at 1.0 V is the vendor's setting and goes through the self-test like the others) |
| `Mid` | 96 MHz | COREPLL N = 16 | `0x0C` | after the level self-test |
| `High` | 144 MHz | COREPLL N = 24 | `0x0C` | after the level self-test |
| `Turbo` | 192 MHz | COREPLL N = 32 | `0x10` (1.1 V) | after the level self-test |
| `Max` | 288 MHz | COREPLL N = 48, `CMU_SYSCLK` bit 8 set | `0x18` (1.3 V) | an option the user can choose (decision D-4, 2026-11-24; WP PWR-5): with Settings › System › Performance = Maximum the governor's step above High is `Max` (§2.4). **Verified 2026-11-24**: self-test PASS, NOR ÷6 (`0x203`) = 48 MHz, compute 1.93–1.94× High, decode bit-identical; battery drain not yet characterised (battery logger, user check 19) |

Voltage codes per the datasheet (01 §7): `0x0C` = 1.0 V, `0x10` = 1.1 V,
`0x14` = 1.2 V (the reset default), `0x18` = 1.3 V. All values are rows of the
vendor's own table; never interpolate. **The first act of the governor is to
lower the core supply from the reset default of 1.2 V to 1.0 V** (code
`0x0C`), which the stock firmware uses for everything up to 144 MHz. `Turbo`
is still below the reset-default voltage. `Max` raises the supply above the
default (within the regulator's documented range and the datasheet's 288 MHz
rating) and is what stock does for UI bursts and recording; Orpheus should
not need it.

### 2.2 Level change (resident, interrupts masked with BASEPRI)

Up: (voltage code if it changes → wait 150 µs) → select HOSC → write COREPLL
`N | 0x80` → wait 100 µs (no lock bit exists) → select COREPLL. Down: select
HOSC → reprogram or stop COREPLL → wait → select → voltage code last. The
detour through HOSC is more conservative than the vendor (who retunes the PLL
under the running core) and costs about 100 µs at 24 MHz.

**Level self-test** (gate for adding a level to the table): at the new level
run a RAM pattern test over the mode pool, a CRC over the whole NOR page
store through the pager's read path, 2 s of the MP3 and FLAC decode benchmark
with output CRC, and an LCD blit with panel read-back. Any mismatch removes
the level.

### 2.3 Peripheral clocks

They do not follow the core clock, with two caveats the drivers must handle:

* **SPI0** can be fed from COREPLL (the ROM's `0x101` setting), DEVPLL or
  HOSC; the encoding of `CMU+0x20` is only partly known (U-PWR-3). The NOR
  driver must run from a source that level changes do not touch: HOSC
  (24 Mbit/s, about 340 µs per 1 KiB line) by default, DEVPLL-derived
  (≈ 45–60 Mbit/s) when DEVPLL is on. The governor calls
  `nor.set_clock(…)` inside the masked section when it starts or stops DEVPLL.
* **LCM**: the vendor switches the core to HOSC for every pixel transfer, so
  the bus timing depends on the core/bus clock (U-LCM-1). Until the timing
  fields are understood the display driver does the same (mux to HOSC, blit by
  DMA, mux back; COREPLL keeps running).
* **DEVPLL** (vendor: always 180 MHz) is off by default in Orpheus and is
  requested by drivers that want it (SD high speed, fast NOR, LCD clock above
  HOSC-derived rates) through the same request mechanism as core levels.

### 2.4 Governor API

```rust
pub enum Level { Idle, Low, Base, Mid, High, Turbo }
pub struct PerfRequest(/* slot */);            // RAII; dropping it withdraws the request
impl PerfRequest { pub fn new(who: Client, level: Level) -> Self; pub fn set(&mut self, level: Level); }
pub fn devpll_request(who: Client) -> DevPllRequest;
pub fn stats() -> ClockStats;                  // seconds per level, seconds in WFI, switches
```

The governor runs the maximum of all requests (not the vendor's sum of
votes), with 200 ms hysteresis on the way down. Policy numbers: 02 §5.1.

**As built (PWR-2 first cut, PWR-5)**: the decision is a pure function,
`orpheus_power::governor::Governor::step` (host-tested), run once a second
by the firmware: idle share of the main executor below 25 % → one level up
(Base → Mid → High); above 70 % for three steps in a row → one level down,
never below the audio task's clock floor. The Performance setting
(Settings › System › Performance, store key `0x0C07`) decides what lies
above High: **Normal** (default) nothing — the governor never selects `Max`,
and a `Max` it finds (a test pin with the governor on, or the setting just
switched off) goes to High at the next step; **Maximum**: `Max` (288 MHz),
with the same hysteresis on the way down (`Max` → High → Mid → Base).
`Turbo` is still only reachable by a test pin.

**WP-PWR-1** delivers levels `Low`/`Base`, then `Mid`/`High` through the
self-test, the request API, statistics, and the SPI/LCM handling above.
Acceptance: `hil power` benchmark scales within 3 % of the clock ratio; no
pager CRC error across 1000 random level switches during playback.

## 3. Core bring-up (WP-HAL-1)

* **crt0** (in `firmware`, and a copy in each bring-up image): mask
  interrupts, clear NVIC enables, stop SysTick, VTOR to our table, SRAM2/3
  bring-up (01 §3), stack pointer, `.bss` zero, fault enables, the ROM-state
  recreation (`PMU+0x00` bits 29/28, COREPLL 48 MHz, `CMU_SYSCLK`, gates,
  bus-master enables), FPU access (CPACR) with lazy stacking, priorities, then
  the pager, then Rust `main`. Model: `crt0-atj2157.S` and
  `system-atj2157.c::atj_pager_early_init`.
* **Low-battery boot guard**: the vendor's second stage refuses to boot below
  battery ADC 683 without USB power; our stages bypass that code, so early
  init does the same check (six consecutive reads) and powers off.
* **Priorities** per 02 §4.1 after U-CPU-1; **critical section** with BASEPRI;
  **RT executor** on a spare NVIC line (U-IRQ-2).
* **Watchdog**: `RTC+0x1C` — bit 0 feed, [3:1] period code (0.176 s … 11.2 s;
  01 §7), bit 4 enable, bit 5 interrupt instead of reset. Run-time: code 5
  (5.6 s). Reboot = code 0 with enable. Consider the interrupt mode as an
  early-warning stage that stores a fault record before the real reset
  (U-WDT-1).
* **Hardware long-press reset**: program `ONOFF_CTL` so holding Power for 8 s
  resets the SoC even if the firmware is dead (01 §7).
* **Reset and reboot reasons**: `reboot(Reason)` writes the boot mailbox and
  lets the watchdog fire; `SYSRESETREQ` does nothing on this SoC.
* **GPIO/MFP**: typed pins (`Input`, `Output`, `OpenDrain` emulated by
  switching the out-enable bit, `Alternate<N>`, `Analog`), resolving U-GPIO-1.
* Bring-up image `hello`: backlight blink from an Embassy timer on both
  executors, result block with timer accuracy against the host clock.

## 4. Timers and the Embassy time driver

Facts: datasheet §4.5.4–4.5.5; system.md §5.3. Four 32-bit timers at
`RTC+0x100/120/140/160` (24 MHz; CTL bit 5 enable, bit 2 reload, bit 1
interrupt enable, bit 0 pending/write-1-to-clear; `+0x04` value: interrupt
after n + 1 clocks; `+0x08` current count; T2/T3 also have capture). Their
clocks are selectable per timer in `CMU+0x40…0x4C`. IRQs 2–5; T0 **[HW]**.
Start-up needs timer reset/gate bit 24, `0xC0000104 |= 1`, `RTC_CTL` bit 17
and the RTC update handshake (`timer_hw_init` in the port).

* **T0: time base.** Free-running with reload value `0xFFFF_FFFF` (179 s at
  24 MHz); the overflow interrupt extends it to 64 bits;
  `now()` reads count and overflow consistently (read twice around the
  pending flag).
* **T1: alarm.** One-shot programmed with the distance to the next deadline;
  its interrupt calls the Embassy timer-queue wake. Deadlines beyond one
  period chain.
* Tick rate: 1 MHz, or 24 MHz if `embassy-time` offers that tick rate (then
  `now()` needs no division). Check the current `embassy-time-driver` trait
  and queue mechanism in docs/research/rust-ecosystem.md §1.
* T2/T3 stay free for profiling (1 kHz PC sampler) and the USB completion
  poll. Their capture inputs are worth a look for key-edge timestamps.
* If a timer can be clocked from the 32 kHz oscillator (`CMU_TIMERxCLK`), one
  of them becomes the wake-up timer for deep idle with HOSC stopped (§10).
* No SysTick: it follows the core clock and would break at every level change.

Acceptance: 10 min drift against the host clock within crystal tolerance
(± 50 ppm) while the governor switches levels continuously; alarm latency
histogram ≤ 100 µs at `Base` with no page faults pending.

## 5. DMA

Facts: datasheet ch. 5 (authoritative for the control word); audio.md §6;
01 §6. One HAL module: channel ownership by type, mode
word builder with named device selectors, address translation for SRAM2/3
(`dma_addr`), start/abort, remaining count, completion as an `async` wait on
the channel's interrupt (IRQ 26 + n), optional auto-reload (U-DMA-2).
Channel 0 is reserved for the pager and is driven by its own resident code,
not through this module.

## 6. NOR flash and paging support (WP-PG-1, WP-PG-2)

Facts: `spinor.h`, `norprog.c`, `pager-atj2157.c` **[HW]**; system.md §9.

* `NorCore` (resident, shared with the pager): init as the ROM does (release
  from power-down `0xAB`, exit continuous-read `0xFF`, discard one
  transaction), JEDEC id check, `read_line` (FAST READ + DMA channel 0),
  status polling, write-enable, page program `0x02` (≤ 256 B), sector erase
  `0x20` (4 KiB, **[HW]**), block erase `0xD8`, suspend/resume (U-NOR-2),
  descrambled read (CTL bit 12) for vendor data.
* `Nor` (async front end, paged): `embedded-storage-async` `NorFlash` over a
  region with compile-time bounds. **Region guards are part of the type**:
  the settings/vault region and the slot region are separate objects; no API
  can address sector 0 or the vendor area in the dual layout. The installer
  payload has its own guarded copy.
* Every program/erase sets the `busy` state for the pager; long erases are
  split into "start, then poll with awaits" so tasks keep running while the
  chip works and page faults suspend it.
* The SPI clock policy of §2.3.
* **WP-PG-1**: `NorCore`, pager on the device (with `xtask pagelink`), slot
  format, stage 2, fault record, crash screen. Acceptance: `hil pager`.
* **WP-PG-2**: program/erase with suspend, settings store, `Nor` front end,
  power-cut tests on the host model, on-device endurance smoke (1000 writes
  to the scratch sector while audio plays, zero underruns).

## 7. Display and backlight (WP-LCD-1)

Facts: `lcd-yotonym03.c`, `backlight-yotonym03.c` **[HW]** (init table, window
commands, panel offsets, PWM4); system.md §1.5 (LCD clock, HOSC switch);
audio.md §6 and `DRV_LCD.DRV` around `0x119A1C–0x119C30` (DMA blit: channel 1
mode `0x9600`, and the special channel at `0xC0071000`).

* `Lcm`: controller init, command/data writes, **DMA blit of a rectangle of
  the frame buffer** (set window, start DMA, await completion), RAM read-back
  for tests (`payloads/lcdread.c`).
* `Panel`: reset, init table, sleep in/out, display on/off, rotation by
  MADCTL (0°/180°), the x+2/y+3 offset (re-derive for 180°).
* `Display` task interface for the UI: `flush(Rect)`, `set_power(bool)`,
  backlight level with fades (PWM duty steps on a timer, no CPU while steady).
* Flush cost targets: full frame ≤ 12 ms, a list row ≤ 2 ms, both with the
  CPU free during the DMA (apart from the HOSC detour of §2.3).
* Byte order: the PIO path sends high byte first; find the DMA mode that does
  the same or keep the frame buffer in panel byte order and convert in `Gfx`
  (U-LCM-2). Screenshots must still decode correctly on the host: record the
  byte order in `orpheus-proto`.
* Bring-up image `lcd`: colour bars, 1-pixel grid, text; read back four
  rectangles from the panel and compare; screenshot via ADFU.

Acceptance: read-back equals the frame buffer for full and partial flushes at
every enabled clock level; backlight levels monotonic (user check for
appearance).

## 8. Keys and earphone sense (WP-INPUT-1)

Facts: `button-yotonym03.c` **[HW]**; system.md §3.3; fm-i2c-imu.md §2.6.

* Enable: key-ADC clock gate bit 27, `PMU+0x38 |= 0x210`, GPIO17 analogue
  (`0x303B`), `0xC01C0304 = 0x3001`.
* Sample both ladders and the power bit every 10 ms from a timer-driven task
  (50 ms with the panel off). A key is accepted after two equal consecutive
  decodes, because a ladder passes through the levels of other keys while
  settling. Thresholds half-way between the measured values of 01 §5.
* Output: `Key` + phase events with timestamps taken at the sample (the
  stopwatch uses them), plus an injected-event path for the developer channel
  and the simulator.
* Earphone sense: when no ladder-1 key is decoded, classify LRADC1
  (385–591 = present) after 11 equal samples.
* Power key: read `PMU+0x2C` bit 1 and write 1 back to clear the latch.
* Diagnostics screen shows raw values (the per-unit spread is unknown).

Acceptance: scripted host tests on recorded ADC traces (including ladder
settling and two-key cases); user check: every key registers once per press,
no ghost keys while inserting earphones.

## 9. SD card (WP-SD-1, WP-SD-2)

Facts: `sdmmc-yotonym03.c` **[HW]** (register use, command types, DMA channel
4, R3 CRC quirk, busy handling by CMD13 polling, 16-block transfers at
24 MHz); system.md §1.5 (`CMU+0x14`: `0x41` identification, `0` = HOSC,
`0x200 | n` = DEVPLL ÷ (n + 1)).

* `SdHost` (HAL): power/clock, bus width, clock select, `command(cmd, arg,
  resp_type)`, `read_blocks`/`write_blocks` with DMA and an `async` wait on
  the DMA interrupt plus controller status; errors mapped (timeout, CRC,
  other); the clock stopped between bursts.
* `sd` protocol module (in `orpheus-fs` or its own crate, host-testable against
  a card model): identification, SDHC/SDXC, CMD6 high-speed switch when
  U-SD-1 passes, CMD18/CMD25 with CMD23 pre-count, CMD12, CMD13 ready polling
  after writes, retry policy (3 tries, re-init on persistent errors), card
  removal detection (GPIO18 and command time-outs).
* Transfers up to 128 blocks per command (64 KiB DMA) once verified; the port
  used 16.
* Block-device trait for the file system and the USB pump with direct-DMA
  buffers (word-aligned, any RAM block).

Acceptance: 100 MiB read/write pattern test inside the scratch file region
with zero errors; sequential read ≥ 10 MB/s at 24 MHz 4-bit (≥ 18 MB/s in
high-speed mode); hot removal during a read returns an error within 500 ms
and re-insertion remounts.

## 10. PMU: battery, charger, power states (WP-PWR-2, WP-PWR-3)

Facts: system.md §3–4 (register map, charger algorithm and thresholds,
power-off sequence, wake registers); `power-yotonym03.c` **[HW]** for
power-off and the battery ADC.

* **Battery**: ADC enable `PMU+0x38` bit 0, raw 10-bit value plus the eFuse
  trim the vendor applies (read-only eFuse access, system.md §5.5); mV ≈ raw ×
  4200 / 900 until calibrated (user check with a meter improves it); gauge per
  02 §5.5.
* **Charger**: reproduce the vendor state machine and values exactly
  (presence detect, 50 mA (code 1) below raw 640, 200 mA (code 4) otherwise,
  full at raw 892 with the phase/current-band test, re-charge below 821).
  200 mA is 0.54 C for this cell: **never select a higher code.** Keep the
  end-voltage fields at the values the stock firmware leaves (read them back
  from a unit that has just run stock; U-PMU-1). The datasheet's hardware
  termination (`ENCHGATDT`, `CHGEND`) may replace the software test once
  observed over a full cycle.
  `ChargeState { NotPresent, Trickle, Charging, Full, Fault }` to the UI.
* **Cable**: `PMU+0x28` bit 30 or `PMU+0x40` > 0x200, debounced; host versus
  charger from the USB driver.
* **Regulator set-up**: the vendor kernel writes `PMU+0x0C = 0x68602990` at
  start and `PMU+0x00[12:10] = 4`; the Rockbox port never did and runs. These
  may select more efficient regulator modes. Apply them as a measured
  experiment (battery-slope comparison, user current measurement if
  available), marked `VERBATIM`, and keep whichever is better.
* **Power-off**: `ONOFF_CTL[8:6] = 2` → `WKEN_CTL = 5` (key long press + DC5V
  insert; add bit 4 when an alarm is armed) → `WAKE_PD = 0x01F00E1F` (clear
  pending) → clear `POWER_CTL` bit 1 → bit 0, 500 µs apart (vendor order).
* **S3 standby** (documented, unused by stock): the candidate for "off but
  alarms and step counting alive" and for instant resume. Everything about it
  is U-PMU-2; if RAM survives and the SoC restarts through the ROM, stage 2
  recognises a "resume" mailbox marker and jumps back into the retained image
  instead of reloading it.
* **Deep idle** (our own state; the vendor has none): by steps, each kept only
  if U-PWR-4 shows a gain and clean wake-up: gate every idle peripheral clock;
  COREPLL off; core at `Idle`; DEVPLL off; optionally the 32 kHz source with
  HOSC-dependent blocks stopped. Wake sources available without unknown
  interrupt lines: timers (key and sensor polling at 10–20 Hz), RTC alarm
  (IRQ 6), USB (IRQ 12, if the controller is left powered) or VBUS polling.
* **Off with alarms**: `WKEN_CTL` bit 4 enables wake by the RTC alarm
  (datasheet; confirm with U-PMU-3). If it fails on this board, pending alarms
  keep the player in deep idle instead of off, and the UI says so.
* Shutdown path, auto-off, sleep timer, low-battery cut-off (raw 683 as the
  vendor, with our own earlier warning levels).

Acceptance: charger states follow a scripted ADC trace on the host model;
**[HIL]** state reporting while plugged; user checks: charge to full
terminates, current never above the stock value (USB meter if available),
power-off and power-on by key, wake by cable.

## 11. RTC (WP-RTC-1)

Facts: system.md §5.1. Binary time `RTC+0x0C`, date `+0x10`, alarm `+0x08`/
`+0x18`, control bits (calendar run bit 4, alarm enable bit 1, pending bit 0),
update handshake `0xA596 → 0x5A69` after every write, alarm IRQ 6.

* API: get/set UTC date-time, set/clear alarm, alarm event; local time is the
  UI's business (UTC offset in settings).
* Year range 2000–2127; reject anything else.
* Validity: with no backup registers, "clock was set" is a settings-store
  flag plus a plausibility check (date ≥ build date); U-RTC-1 looks for an
  oscillator-stopped indication.
* Drift correction applied in software from the stored seconds-per-day value.

Acceptance: set/read round trip; alarm fires within 1 s; 12 h drift logged
against the host.

**Alarm as built (UI-5d, 2026-11-24)**: the UI arms the RTC alarm
(`rtc_clock::set_alarm`) with the next enabled alarm's time whenever the
alarms or the minute change, and clears it when none is on;
`rtc_clock::alarm_state` reads it back (`alarm status`: armed time,
enable, pending). While the player is on, the alarm rings from the UI's
once-a-second check of the RTC minute (`orpheus_apps::tools::Tools::poll`,
host-tested), not from the pending bit. **Not done (PWR-3c)**: waking from
off. `services::ALARM_WAKE_FROM_OFF` is `false`; with it `true`, power-off
would pass `pmu::power_off(true)` (`WKEN_CTL` bit 4) while an alarm is
armed. Missing before it can be switched on: U-PMU-3's alarm case on this
board (arm the alarm 1 min ahead, power off, watch USB re-enumerate), and a
way for the start to know it was the alarm (wake reason) so the alert
rings at boot. The Alarm tab says "Rings only while on" meanwhile.

## 12. DAC, headphone amplifier, mixer (in WP-AU-1)

Facts: `pcm-yotonym03.c` **[HW]**; audio.md §1–2, §4, §6.3.

* Power-up exactly as the port's `dac_power_up` (the vendor's direct-drive
  anti-pop sequence), then add: power-down sequence (audio.md §2.5), mute,
  digital volume registers `+0x14`/`+0x18` with the soft-ramp bits, amplifier
  gain `+0x28[14:12]`, rate setting through `CMU+0x88` (table in audio.md
  §1.3), FIFO/DMA start and stop, the analogue input bypass switches
  (`+0x2C` bits 16–23) for FM and line-in.
* Volume mapping: user steps → digital volume code with the amplifier gain at
  its music value (7); the vendor's 32-entry table (audio.md §2.3) is the
  reference for the range; Orpheus uses 41 steps of equal dB inside the same
  range once U-AUD-1 gives the scale.
* Stream: DMA channel 5, 16-bit stereo mode; auto-reload over the PCM ring if
  U-DMA-2 passes, else chained transfers with the pager poll hook.
* `AudioOut` trait towards `orpheus-audio`: `start(rate)`, `stop()`,
  `position()`, `set_volume()`, `mute()`, `route(Bypass)`, statistics.

## 13. ADC and inputs (in WP-AU-13)

Facts: audio.md §3–4. Block at `0xC0181000`: input select (microphone
single-ended/differential, FM, line), analogue gain `+0x44[11:8]`, digital
gain `+0x24/+0x28[6:0]`, capture FIFO `+0x18`, DMA mode `0x0014008B`, rate
through `CMU+0x80`. Everything is **[code]**: the bring-up image records each
source to a WAV on the card for inspection on the host (U-AUD-4). Mono
microphone capture is the vendor default; stereo ADC is gated by IC type.

As built (AU-13): `atj2157_hal::adc` — `start(input, rate, analog,
digital)`, `set_gain`, `stop`, `regs`; the vendor's API 18 sequence VERBATIM
with the datasheet's register names, except that the stop keeps `CLKEN1[0]`
and puts `CMU+0x80` back to the DAC power-up's value (the vendor writes 0,
which would silence analogue output) and the rate function also clears
`CMU+0x80` bit 4. The DMA runs on channel 6 in reload mode over a two-half
ring; `atj2157_hal::dma` counts its half/complete events in a handler of its
own (`enable_ring_irq`, `events`, `wait_events`; resident with the rest of
the module). No PMU access. Open: U-AUD-4, U-AUD-11–13.

After the first device run (microphone silent): the microphone front end
now follows the vendor's left side (`MIC_MIX_CTL[3:2]`, FD buffer L in
`CTL`; 01-hardware §8, U-AUD-13), and `start` takes a `Width`: 16-bit
(the vendor's, mode `0x0014008B`) or 32-bit words (`FIFO_DMAWIDTH` = 0,
mode `0x0004008B`) narrowed by the caller (`orpheus_rec::pcm`, U-AUD-12).
`fifo_peek` reads `FIFOSTA` and two FIFO words for `rec raw`;
`dma::remaining_of` reads a channel's REMAIN without its token.

## 14. DSP accelerator (WP-DSP-1)

Facts: audio.md §7.1. A block at `0xC0200000` offering real FFT/IFFT (128,
256, 512 points; 16- or 32-bit), a biquad cascade with externally held state,
and a third stateful filter with eight modes and variable output length. The
vendor driver contains the code; **nothing in the stock firmware calls it**,
so it may be unfinished or absent on this die.

Plan, in this order, stopping at the first failure:

1. Clock/reset per the vendor entry, read the control register, run one FFT
   of a known vector through DMA; compare with a software FFT to find the
   scaling and bin order (U-DSP-1).
2. Biquad: impulse through one section with unit coefficients to find the
   coefficient layout and Q format; then a 5-section EQ against the software
   reference.
3. Third function: characterise the eight modes with sweeps (is it a
   resampler? which ratios?).
4. Measure the end-to-end cost including DMA set-up for typical block sizes
   (one PCM block of 1152 stereo samples through 5 sections).

Use it where it beats the Cortex-M4 on energy or time: the EQ, the spectrum
display, the decimator/resampler if function 3 is one, and — if the FFT is
fast enough including transfer overhead — as the kernel of the IMDCT in the
Vorbis/AAC/Opus decoders (behind a trait, with the software path always
present). If it does not work, record that in audio.md and close the package;
nothing else depends on it.

## 15. I2C, FM tuner, motion sensor (WP-FM-1, WP-SENS-1)

Facts: fm-i2c-imu.md.

* **I2C**: two implementations behind one `embedded-hal-async` `I2c`, shared
  by both devices through an async mutex.
  1. *Software master* on GPIO24 (SCL) / GPIO25 (SDA) with open-drain
     emulation (drive low or release; never drive high), clock-stretch
     tolerant, bus recovery (9 clocks). This is what the stock firmware does
     and is the known-good path for the first scan.
  2. *Hardware TWI1* (`0xC0120000`, datasheet §7.5, IRQ 20, DMA selectors
     0x14): the pin list gives GPIO24/25 the TWI1 functions, so the controller
     can run the bus with interrupts instead of busy-waiting (U-FM-1). If it
     works it becomes the default (400 kHz) and the software master stays as
     the recovery path.
* **Tuner (RDA5807 family at `0x10`)**: probe and id (U-FM-1), init table per
  variant with the 24 MHz reference setting (`02h = 0xC451`), tune, hardware
  seek with threshold, mute, mono, band and spacing, de-emphasis 50/75 µs by
  region (the stock firmware leaves 75 µs everywhere), RSSI, stereo flag,
  power-down. Avoid 95.9–96.1 MHz in auto-scan (reference harmonic). RDS:
  only if the id says an RDS-capable variant; then group decoding for PS, RT
  and CT in `orpheus-apps::radio` with host tests on recorded group streams.
  Audio: the analogue bypass of §12; volume = tuner volume × amplifier gain
  (the vendor's 32-entry pair table is the starting point).
* **Accelerometer (MiraMEMS DA2xx family at `0x27`)**: id read (U-ACC-1),
  init per fm-i2c-imu.md §3.2 (± 2 g, 50 Hz class), burst read of six data
  bytes at 50 Hz while a consumer is active (25 Hz for background step
  counting if accuracy holds), suspend (`0x11 ← 0x80`) otherwise and at boot.
  If the identified part has a FIFO, a step counter or a usable interrupt
  pin, use them (fewer wake-ups); the baseline assumes none.
  **As built (SENS-1, 2026-11-24)**: `yoton_bsp::accel` on the software
  master at 100 kHz; open = the vendor's functional steps (± 2 g, ODR code
  6 = 62.5 Hz, unlock, normal mode; **VERBATIM**), suspend at boot and
  whenever nothing counts; one 6-byte burst read per sample. The `sensor`
  task (main executor) samples at **25 Hz** while counting is wanted (Count
  in background "While on", the Steps app open, or a trace) and runs the
  detector, the day check and the history once a second over the batch;
  the per-sample code is the `steps-sample` cluster of `hot.toml` (4 lines;
  the read path touches about 2). No FIFO and no interrupt (U-ACC-1 open:
  `acc regs` dumps the map). State in SRAM3 right after the battery
  logger's (`sensor::SHARED_ADDR`, `0x307900` as built; the radio's
  follows, the three chained by asserts). Developer commands: `acc read`, `acc regs`,
  `acc trace <secs>` / `acc trace stop` (CSV to
  `/.orpheus/test/acc-<n>.csv`, `orpheus_steps::trace`), `steps status`.
* **Step detector** (`orpheus-apps::steps`, host-tested): magnitude of the
  3-axis vector, low-pass, adaptive threshold between running max and min,
  minimum 200 ms and maximum 2 s step interval, a "regularity" gate of four
  consecutive plausible steps before counting starts (suppresses bus rides
  and handling), as an improvement on the stock algorithm (fm-i2c-imu.md
  §3.4, which is the fallback reference).
  **As built** in the crate `orpheus-steps` (`detector`, integer only):
  gravity removed with a ≈ 1.3 s average, ≈ 4 Hz low-pass, decaying
  envelopes with ± ¼-swing hysteresis, minimum swing by Sensitivity (0.16 /
  0.10 / 0.06 g), step interval 270 ms – 2 s, four regular candidates
  (intervals within 40 %) before counting; a candidate whose interval holds
  two rises of the unfiltered signal counts as too fast (shaking that the
  low-pass halves into a walking rhythm), two too-fast ones without a
  clean step between them drop the walk. Tests (`tests/synthetic.rs`,
  25 and 50 Hz): walking, slow, brisk, running, pocket, clipped sprint
  within ± 5 %; 0 steps on a table for 10 min, handled, on a bus, shaken
  at 3.8–8 Hz. Shaking slower than ≈ 3.7 Hz is a running cadence and
  counts. `traces/*.csv` with `# truth=N` are replayed (`tests/traces.rs`).

Acceptance: I2C scan finds exactly `0x10` (and `0x11`/`0x60` on some
variants) and `0x27`; tuner tunes and reports plausible RSSI on a local
station; accelerometer reads 1 g ± 10 % at rest on each axis orientation
(user check: turn the player); step traces per 05 §7.7.

**As built (FM-1, 2026-11-24, not yet run on the player).** Tuner logic in
the host crate `orpheus-fm` (register model, vendor tables, seek, scan,
RDS PS/RT, presets, the radio service state machine; tested against a
register-level fake). Firmware task `fm` (main executor) drives it over the
software master at 100 kHz (writes only to `0x10`); TWI1, the async bus
mutex and CT (RDS clock) are not done: nothing else uses the bus until
SENS-1. The analogue route is `atj2157_hal::dac::fm_precharge/connect/
release/disconnect` (the vendor's steps without its amplifier power cycle);
volume is the vendor's pair table (`orpheus_fm::volume`). Region Wide is
65–108 MHz (the tables select 65 MHz for `BAND = 11`). The tuner is in
standby whenever the radio is not playing, and is put there at boot.
Developer commands: `fm on|off|status|scan`, `fm tune KHZ`,
`fm seek up|down`, `fm route on|off`.

## 16. UART (in WP-BT-1)

Facts: system.md §7; bluetooth.md §2. UART1 at `0xC00E0000` for the Bluetooth
module (DMA in both directions, baud = 24 MHz / divisor in both halves of
`+0x10`). UART0 at `0xC00D0000` is a debug port whose TX pad is disabled in
the shipped configuration and shared with SD detect on this board: not used
by Orpheus (logging goes through the developer channel).

## 17. USB device controller

Specified in 06-connectivity §2 (driver) with `usb_controller.md` and the
port's `usb-atj2157.c` as sources.

## 18. Work-package index for this document

| WP | Content | Depends on |
|---|---|---|
| HAL-0 | `atj2157-pac` from the datasheet (01 §1), with trust tags | T-1 |
| HAL-1 | crt0, priorities, critical section, executors, timers/time driver, watchdog, GPIO, `hello` image | T-1, T-2 |
| LCD-1 | Display, panel, backlight, DMA blit, read-back | HAL-1 |
| INPUT-1 | Keys, power key, earphone sense | HAL-1 |
| PG-1 | NOR core, pager on device, slot format, stage 2, faults, crash screen | HAL-1, T-3 |
| PG-2 | NOR writes with suspend, settings store | PG-1 |
| SD-1 / SD-2 | SD host + protocol / high-speed mode | HAL-1 (DMA) |
| FS-1 | File system (02 §6) | SD-1 |
| PWR-1 | Clock governor and levels | PG-1, LCD-1 |
| PWR-2 | Battery, charger, cable, regulator experiment | HAL-1 |
| PWR-3a / 3b / 3c | Shutdown, wake and auto-off / screen-off policy and deep idle / S3 standby (split as scheduled in 08) | PWR-1, PWR-2, RTC-1 |
| RTC-1 | RTC and alarms | HAL-1 |
| FM-1 | I2C, tuner, analogue route | AU-1 (amplifier) |
| SENS-1 | Accelerometer, step detector | FM-1 (I2C) |
| DSP-1 | Accelerator investigation and integration | AU-1 |
| HAL-9 | PAC reconciliation: go through the datasheet chapter by chapter and correct names, fields and reset values in the PAC and in `docs/hw/*.md` | HAL-1 |
| XIP-1 | Time-boxed investigation of the hardware SPI cache (01 U-XIP-1) | PG-1 |
