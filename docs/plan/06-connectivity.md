# 06 — Connectivity: USB, Bluetooth, authenticator

Hardware sources: `/home/tim/Documents/GitHub/yoton/re/usb_controller.md` and
the verified driver `ext/rockbox/firmware/target/arm/atj2157/usb-atj2157.c`
for USB; [docs/hw/bluetooth.md](../hw/bluetooth.md) for the Bluetooth module;
[docs/hw/fm-i2c-imu.md](../hw/fm-i2c-imu.md) §3 for the motion sensor.

## 1. What the hardware allows (read this before promising a feature)

| Wish | Verdict | Why |
|---|---|---|
| USB card reader, high speed | **Yes** | Proven by the Rockbox port (2.6 MB/s unoptimised) |
| USB disk / CD-ROM emulation from a file | **Yes** | Same controller, more SCSI; needs the extent API of the file system |
| USB keyboard (typing one-time codes) | **Yes** | Interrupt-IN endpoints; needs interrupt-type FIFO configuration to be tested (U-USB-11) |
| USB audio device (sound card: playback and microphone) | **Probably** — milestone M16 (owner, 2026-11-25) | Needs isochronous FIFOs with the Actions DMA and an SOF/frame counter for rate feedback; untested (U-USB-12, U-USB-15) |
| USB host / OTG | **No (not planned)** | The core is a dual-role Faraday design, but nothing in the vendor image uses host mode and there is no sign of a VBUS supply |
| Bluetooth: send music to headphones/speakers | **Yes, cheap** | The board carries an autonomous transmitter module (self-reports "ATS2851") that runs its own stack and takes **analogue** audio from the DAC; the player only sends short UART commands |
| Bluetooth: remote keys and volume from the headset; volume and track text to the headset | **Yes** | In the module's command set |
| Bluetooth: receive audio and play it through the DAC | **Out of scope** (owner's decision, 2026-11-23) | The module's stock firmware has no such command and no audio route from the module back into the SoC is known |
| Bluetooth: HID (presentation remote, tilt pointer, code typing) | **Under investigation, promising** | The module's SDP record lists HID (0x1124); keyboard + consumer reports, no mouse; the command that feeds it is not known yet (§4.3); the pointer only if Bluetooth can carry mouse data (D-10) |
| Bluetooth: BLE | **No with the module's stock firmware** | No such command |
| One-time codes (TOTP/HOTP) on screen | **Yes** | Needs a correct clock |
| Typing codes into a computer | **Yes over USB HID**; over Bluetooth if §4.3 succeeds | |
| Presentation remote | **In scope** (owner, 2026-11-24): over Bluetooth HID (§4.3) | App: 05 §7.12 |
| Mouse / pointer control | **Conditional** (D-10): only if a Bluetooth transport carries mouse data; otherwise dropped | 3-axis accelerometer (MiraMEMS DA2xx), no gyroscope: pointer by tilt with a clutch key (§4.3.2) |

Bluetooth HID depends on the fitted module, not on this plan: §4.3 stages
the work: the presenter remote is a Bluetooth remote; the tilt pointer is
built only if a Bluetooth transport can carry mouse data (owner's decision
D-10, 2026-11-24). The receiver role and BLE stay out of scope.

## 2. USB device stack

### 2.1 Controller facts that shape the design

* Faraday FOTG210-style device core at `0xC0140000` with an Actions DMA engine
  (five channels: endpoint 0 and FIFOs 0–3). **At most four non-control
  endpoints**, fewer when a FIFO is double-buffered.
* All data, including the 8-byte SETUP packet, moves by DMA from/to
  word-aligned buffers in SRAM0. One DMA start can carry up to 64 KiB and the
  hardware packetises it (ROM-verified in both directions for exact-length
  transfers).
* The core runs the control status stage itself (`CX_DONE`), applies
  SET_ADDRESS itself, and offers stall bits per endpoint.
* High speed is negotiated automatically; `DMCR` bit 6 reports it.
* Interrupts: one line (IRQ 12) for bus events, SETUP, and FIFO state. DMA
  completion is polled in the vendor code; whether it can interrupt is open
  (U-USB-3).
* VBUS: `PMU+0x28` bit 30 or `PMU+0x40` > 0x200. Host versus charger: attach
  and look for start-of-frame counts, as the stock kernel does.
* Power-up needs the ROM's sequence (PMU bits 29/28, `0xC0000114`-era clock
  set-up, `0xC0040004` bus-master enables) after a cold boot; see the port's
  `usb_init_device` and `atj_rom_usb_clock_setup`.

Open questions U-USB-1 … U-USB-12 are listed in 01-hardware with their tests;
the ten from `usb_controller.md` §10 keep their numbers.

### 2.2 `embassy-usb-driver` implementation (`atj2157-hal::usb`)

`embassy-usb` provides enumeration, descriptors and the class framework; the
HAL implements its driver traits. Mapping:

| Trait item | Hardware |
|---|---|
| `Driver::alloc_endpoint_in/out` | Assign a FIFO (0–3) and endpoint number; record type and max packet size; refuse when FIFOs run out. Bulk IN may claim two FIFOs for double buffering when the personality leaves room |
| `Driver::start` | Controller bring-up; returns `Bus` and `ControlPipe` |
| `Bus::poll` | Awaits a `Signal` set by the IRQ 12 handler: reset (re-program endpoint/FIFO maps, as `bus_reset()` in the port), suspend, resume; VBUS polled every 250 ms while waiting (`PowerRemoved` after 0.75 s without; §2.6) |
| `Bus::enable/disable` | Soft attach/detach (`PHYTMSR` bit 0), PHY suspend, clock gate |
| `Bus::endpoint_set_stalled/is_stalled` | Bit 11 of the endpoint's max-packet register; pulse bit 12 on clear (toggle reset) |
| `ControlPipe::setup` | Wait for `CX_SETUP`, read 8 bytes with DMA channel 0 |
| `ControlPipe::data_in/data_out` | DMA channel 0, chunked to the control max packet size; `first`/`last` flags drive when `CX_DONE` is written |
| `ControlPipe::accept` | `CX_DONE` |
| `ControlPipe::reject` | `CX_STALL` |
| `ControlPipe::accept_set_address` | Write `DAR`, then `CX_DONE` |
| `EndpointIn::write` | One DMA per call (≤ 64 KiB), await completion; zero-length packet per U-USB-2 |
| `EndpointOut::read` | Wait for the FIFO's "packet present" interrupt, read the byte count, DMA exactly that many bytes (the vendor method; always correct). Fast path for the storage classes: `read_exact_dma(len)` — one DMA for a known-length data phase |

Completion waits are `async`: the DMA status is checked from the IRQ handler
when U-USB-3 shows a usable interrupt, otherwise from a 1 kHz timer that runs
only while a transfer is in flight. Buffers handed to the driver must be in
SRAM0 and word-aligned; the driver bounces through a 512-byte buffer if not
(log a warning in development builds: a bounce on a data path is a bug).

The table is the intent, written against the released `embassy-usb-driver`
0.2 traits. Embassy's `main` branch has since changed them (the control pipe
merged into the bus, transfer-level endpoint methods) and gained in-tree
mass-storage, MTP and UAC2 classes (docs/research/rust-ecosystem.md §1, §8).
Pin one Embassy git revision for the workspace, write the driver against that
revision's traits, and start the storage class from the in-tree one (adding
the CD-ROM command set) unless it cannot do the zero-copy pump of §2.4.

### 2.3 Personalities

The endpoint limit makes USB functions mutually exclusive sets. The `usb`
service owns the controller and switches personality by soft-detaching,
rebuilding the `embassy-usb` device and re-attaching.

| Personality | Interfaces | Endpoints used | Notes |
|---|---|---|---|
| Charge only | none | 0 | Stay detached after host detection; keeps the card and playback available |
| **File transfer** | Mass storage (card) | bulk IN ×2 FIFOs (double-buffered), bulk OUT | Card unmounted from the firmware while exported |
| **Disk image** | Mass storage (image file as disk or CD-ROM) | as above | §3 |
| **Keyboard** | HID boot keyboard (8-byte report) | 1 interrupt IN | Authenticator typing (§5). Playback keeps running |
| **USB sound card** (M16, §2.7) | UAC2 (UAC1 for older hosts): speaker + microphone, HID consumer control | iso OUT, iso feedback IN, iso IN, interrupt IN | Host audio out to the jack and Bluetooth; microphone in to the host |
| Recovery | — | — | "Recovery mode" reboots into the ROM's ADFU |

Every personality also answers the **developer channel** on endpoint 0 (§2.5),
so tools work regardless of what the player is presenting.

Identity: the Rockbox port borrows Actions' vendor id (`10D6:1157`). Orpheus
uses the pid.codes test id `1209:0001` during development and a product id
requested from pid.codes for release (decision D-3 in PLAN.md). Strings:
manufacturer "Orpheus", product per personality, serial from the eFuse-derived
device id (read-only use of the eFuse words; docs/hw/system.md §5.5).

### 2.4 Mass storage (card reader)

Bulk-only transport with the SCSI transparent command set: INQUIRY, TEST UNIT
READY, REQUEST SENSE, READ CAPACITY (10/16), READ (10/12/16), WRITE (10/12/16),
VERIFY (10), MODE SENSE (6/10), PREVENT ALLOW MEDIUM REMOVAL, START STOP UNIT
(eject), SYNCHRONIZE CACHE, READ FORMAT CAPACITIES, REPORT LUNS. Plus the two
Actions vendor commands the existing host tools use (`CC` identify, `CB 21`
reboot to ADFU) so `yoton.py` keeps working.

Data pump (`usb::pump`, RT executor) with the two 48 KiB buffers of the USB
mode pool:

* read: card → buffer A by SD DMA while buffer B goes out by USB DMA, then
  swap; read-ahead continues past the end of a request when the next LBA is
  sequential;
* write: USB → buffer by one exact-length DMA per 48 KiB, card write (CMD25
  with pre-declared block count) of the previous buffer in parallel; write
  errors surface as CHECK CONDITION on the same or the next command;
* SD at high-speed timing if the card and controller agree (03 §"SD"), core
  clock at the `High` level, DEVPLL on.

Targets on the reference card: ≥ 10 MB/s sequential read, ≥ 5 MB/s write,
data-integrity test of 1 GiB random data in both directions without error.

**As built (PERF-SD rounds 1–2, 2026-11-27).** Two 32 KiB halves of the
mode pool (64 KiB); the host sends READ/WRITE(10) of ≤ 120 KiB. A data
phase runs in one poll of the USB task: the bulk DMA of one half starts
(`EndpointIn::start`/`EndpointOut::start`), the card reads or writes the
other half spinning (no executor round trip: those reloaded the code lines
of the USB task's `select` each time, ≈ 90 % of the time in round 1), then
the bulk DMA is waited for; the next CBW is waited for spinning (≤ 2 ms),
and the transport returns to the executor once every 100 ms of commands
(the UI and the watchdog run then). A sequential READ is read ahead by
one half. The card runs at 48 MHz high speed (U-SD-1, checked by a
read-back, 24 MHz if a card refuses) with the core capped at Mid
(`governor::CARD_HS_CAP`: writes fail at Max); its busy waits poll DAT0
(U-SD-4). The code is the `usb-msc` cluster of `hot.toml`. Measured with
`cargo xtask dev msc-bench` (host direct I/O through files, floors in
`budgets.toml [msc]`): **14.0 MB/s read, 7.6 MB/s write**, 1 GiB written
and read back identical. No pre-declared block counts (CMD23) yet: each
32 KiB is one CMD18/CMD25 with CMD12.
Medium removal mid-transfer returns NOT READY / MEDIUM NOT PRESENT and never
hangs the bus. "Read-only card reader" setting reports write-protect in MODE
SENSE and rejects writes.

### 2.5 Developer channel (endpoint 0, vendor requests)

*Settings › USB › Developer channel (VER-13).* On by default. Off, the
developer personality enumerates with no interface, the card reader,
keyboard and disk-image personalities without the vendor interface, and no
vendor request is answered (no handler). It stays on whatever the setting
says while a host test runs (the test mailbox, `ui test-look on`, a settings
test session), and `settings test-set` refuses it. A change re-enumerates
the developer personality at once. Ways back: Settings › USB › Developer
channel on (the keys), Settings › System › Factory reset; recovery (ADFU:
Settings › System › Recovery mode, the power menu, or reset + Next) works
either way.

`bmRequestType` vendor | device; `bRequest`:

| Req | Dir | Function |
|---|---|---|
| 0x01 INFO | IN | Protocol version, build id, layout, personality, uptime, battery, state flags |
| 0x02 LOG | IN | Read from the defmt ring (`wValue` = cursor) |
| 0x03 FRAME | IN | Read frame-buffer bytes (`wIndex:wValue` = offset) |
| 0x04 KEYS | OUT | Inject key events (key, phase, duration) |
| 0x05 STATS | IN | Named counter groups: pager, audio, SD, USB, power, stack, pool |
| 0x06 TEST | OUT/IN | Test mailbox: start a named HIL test, poll its result block |
| 0x07 REBOOT | OUT | Normal, ADFU, stock firmware, other slot |
| 0x08 TIME | OUT/IN | Set / read RTC (UTC) — available in release builds; the authenticator depends on it |
| 0x09 SET | OUT/IN | Read / write a setting by id (development builds) |
| 0x0A PEEK | IN | Read memory or a register (development builds only) |
| 0x0B PROVISION | OUT | Add an authenticator entry; accepted only while the vault is unlocked and after an on-screen confirmation |

Release builds keep 0x01, 0x07 (normal and ADFU only, with on-screen
confirmation), 0x08 and 0x0B. Control transfers larger than one 64-byte packet
depend on U-USB-2; until it is settled the host library reads in 64-byte
pieces (a screenshot is then 512 requests, about half a second).

A Microsoft OS 2.0 descriptor set (WinUSB) is included so the channel works on
Windows without a driver; on Linux a udev rule for the product id goes into
`tools/udev/`.

**File uploads (as built, PERF-SD round 2, 2026-11-27).** In the developer
personality the vendor interface has one bulk OUT endpoint (`0x01`,
`orpheus_proto::devchan::upload::BULK_EP`). An upload is `fcreate PATH`
(reply `put 0`; only under `/.orpheus/`, or the demo tree with
`fcreate-demo`), then `fput N` (reply `fput N`): the next `N` bytes on the
endpoint go by DMA straight into the two 16 KiB staging halves while the
card task writes the other half (`upload::pieces`); the host waits for
`put N` and sends `fclose` (`closed N`). `cargo xtask dev put` uses it when
the endpoint is there and the 1 KiB `REQ_PUT` control chunks otherwise.
8 MiB in 6.5 s (1.3 MB/s, card-bound) against ≈ 45 KB/s with chunks.

### 2.6 Robustness and recovery (bug USB-W1, as built 2026-11-25)

**The failure (first soak, 2026-11-24).** On a `volume` action the host's
`TEST` OUT timed out after 2 s (`Cancelled`); the next request, a `PEEK` of
the fault record, got `Stall`; from then on no SETUP was answered, not even
after re-plugging on two ports (xHCI "device not responding to setup
address", -71: the SETUP transaction itself got no answer). The UI ran on,
the watchdog was fed, a reboot cured it.

**Root cause (analysis without the player; ranked).**

1. *The SETUP read is not cancel-safe* (certain as a mechanism, the one
   that makes the failure permanent). embassy-usb's `run_until_suspend`
   is `select(bus.poll(), control.setup())`: when a bus event (reset,
   suspend, resume) wins, the `setup()` future is **dropped**. The driver
   cleared `SETUP_READY` and kept the SETUP interrupt masked (`GIMR0` b0,
   masked by the ISR because `CX_SETUP` is level) until the DMA read had
   finished; a drop in between left it masked for good. No later SETUP is
   ever read; a bus reset (re-plug) did not unmask it and a VBUS cycle did
   not re-initialise the controller, so nothing short of a reboot helped.
   The host retries after errors with resets, which is exactly what drops
   a read in progress. Reproduced with embassy-usb itself over a model of
   the hardware state: `crates/orpheus-usb/tests/ep0_model.rs`
   (`before_w1_a_bus_event_during_the_setup_read_deafens_ep0_for_good`).
2. *A late data or status stage lands on the next transfer* (explains the
   `Cancelled` → `Stall` step). The controller has no notion of which
   transfer `CX_DONE`/`CX_STALL` or an EP0 DMA belongs to. When the host
   gives up on an OUT whose data the device has not read yet and sends the
   next SETUP, the waiting OUT DMA drains the CX FIFO — now the new SETUP —
   as the old request's data, and the old request's `CX_DONE` finishes the
   new one: the host's IN sees a status stage where it wanted data. Model
   test `before_w1_a_late_status_lands_on_the_next_transfer`. The FOTG210
   flags the abort in `GISR0` b5 `CX_COMABT`, which the driver never
   cleared (the ROM only clears it at init; the vendor never overlaps
   control transfers).
3. *What made the device late* (trigger; not determined without the
   player). The main executor or the USB task held for more than 2 s.
   Candidates: a stretch of BASEPRI-masked NOR work or paging at 48 MHz
   while the volume overlay redraws and music decodes (the soak runs a
   test session, so no settings write is expected); a 500 ms EP0 DMA
   timeout per chunk of a stage the host had already abandoned. The new
   test hook `usb delay` reproduces the effect of any such delay.
4. *Smaller defects found on the way* (fixed): `dma_wait` failed every DMA
   while the sticky `BUS_EVENTS` reset bit waited for `Bus::poll`, and a
   SETUP read that kept failing retried without yielding (a possible
   executor hang); DMA aborts only wrote the channel's control word (the
   vendor writes `DMA_EN = 0`); a bulk OUT read pending at a bus reset was
   never woken (card reader dead after a reset until a personality
   switch); VBUS removal was never reported; a SETUP while embassy-usb
   believes the bus suspended was never read if the resume interrupt was
   missed.
5. *Ruled out.* A panic in the USB task (the panic handler reboots the
   whole player; it did not reboot). PEEK of a bad address (a fault
   reboots; a bus stall would starve the watchdog; the soak peeks SRAM).
   PEEK length handling (`min(wLength, 1 KiB)`, host-tested). LPM (the
   USB 2.0 extension descriptor advertises none).

**Driver fixes** (`atj2157-hal::usb`): the SETUP read holds a guard that
unmasks the interrupt and aborts the EP0 DMA however it ends (also on
drop); a bus reset re-arms EP0 (clears `SETUP_READY`, unmasks) and wakes
every endpoint future; data and status stages check for a newer SETUP
before and after each DMA and before `CX_DONE`/`CX_STALL`, and give the
old transfer up (`ERR_OVERTAKEN`); `CX_COMABT` is cleared (W1C, as the
FOTG210 driver does) and counted when a SETUP is read; aborts are the
vendor's (`DMA_EN = 0`, channel cleared); DMA waits end on a reset by
epoch, not on the sticky bit; a failed SETUP read yields before retrying;
`Bus::poll` reports `PowerRemoved` after VBUS has been absent for
0.75 s and the next `enable` re-initialises the controller completely
(resets pulsed, init sequence); a SETUP while suspended is reported as a
resume.

**Self-recovery** (`orpheus_usb::liveness`, host-tested; run once a second
by the firmware's `watchdog` task, which runs whatever the USB task does):

| Evidence (VBUS present) | Action |
|---|---|
| A bus reset not followed by a SETUP within 2 s | Recovery |
| A SETUP latched but unread (or its read failing) for ≥ 2.5 s | Recovery |
| A recovery asked for, not carried out within 10 s | Wedged |
| SETUPs unread again after 2 recoveries without a SETUP answered | Wedged |
| No VBUS, a charger (no resets, no SETUPs), an idle host | Nothing |

A recovery ends the running personality's `select` (`REINIT`), the
controller is shut down (detached) and the same personality starts on a
fresh controller, as a personality switch does (a USB Drive image ends
instead; the card comes back). Consecutive recoveries back off (0, 5, 15,
30, 60 s). Unanswered resets alone never escalate: a port can look like
resets.

**Watchdog** (decided for, with limits): `wdt::USB_WEDGED` stops
`Health::check` from feeding only when re-initialising cannot help (the
two "Wedged" rows), with VBUS present, after this boot answered at least
one SETUP, and after 60 s of uptime (past the healthy-start mark, so the
reboot never counts as a failed start for stage 2's fallback). Unplugging
or a SETUP answered clears it. Reasons for: the only cure seen on the
player was a reboot, the soak must run unattended, and a USB task that
does not act on a recovery request is as stuck as a hung executor.
Reasons for the limits: never reboot an unplugged or charging player, or
one whose host is simply idle.

**Observability**: `STATS` group 0 words 0–2 (resets, SETUPs, DMA errors)
and 24–30 (`orpheus_proto::devchan::usb`: stalls, given-up transfers,
recoveries, last error code and time, controller initialisations,
`CX_COMABT` seen, liveness state); `usb: recovery (…)` and `usb: wedged`
lines in the log ring; `xtask dev stats` prints them; the soak prints them
on a channel loss and records a self-recovery as a `usb-recovery` finding
that does not stop the run.

**Provoking it**: test command `usb delay setup|data|status <ms>` holds the
next control transfer at that stage (a timer in the USB task, nothing
blocked); `usb recover` runs a recovery. `xtask dev usb-cancel [--rounds
N]` holds a request 2.5 s at each stage, sends the PEEK the soak sent, cuts
40 transfers after 1 ms, and requires INFO to answer again within 20 s
each time. Firmware before USB-W1 has no `usb delay`; its `usb-cancel`
rounds fail at the first held request, the 1 ms cuts still run.

### 2.7 USB sound card (M16; owner's requirement 2026-11-25)

The player presents a **USB audio interface**: the host's audio output
plays through the player — to the headphone jack (TRS) and, when a
Bluetooth device is connected, to Bluetooth (the module takes the DAC's
analogue output, 06 §4.1, so both follow the same stream) — and the
player's **microphone** appears to the host as an audio input.

* **Class**: USB Audio Class 2.0 at high speed (built-in drivers on Linux,
  macOS and Windows 10 1703+); a UAC1 full-speed variant as a setting for
  older hosts. Composite with the developer channel (always) and a HID
  consumer-control interface (the player's keys as play/pause, next,
  previous, volume on the host). The card stays with the player (no mass
  storage in this personality); music playback stops while the sound card
  is active.
* **Playback stream (host → player)**: stereo, 44.1 and 48 kHz (both PLL
  families; 96 kHz if U-AUD-2 allows), 16- and 24-bit (24-bit dithered to
  the DAC's width unless it takes 24 bits). **Asynchronous** endpoint with
  an explicit feedback endpoint: the DAC keeps its own clock (measured
  +1,500 ppm against the workstation, U-AUD-10) and the feedback value is
  computed from the DAC DMA position against USB start-of-frame counts, so
  nothing is resampled; an adaptive resampler (the DSP chain's) is the
  fallback if the controller cannot give frame timing (U-USB-15).
* **Capture stream (player → host)**: the microphone (AU-13 front end, the
  left channel as found), mono or duplicated stereo, 16 and 48 kHz,
  16/24-bit; packet sizes vary by one sample to follow the ADC's clock.
  Setting for the source: microphone, FM radio, or a monitor of the
  playback (loopback, for tests).
* **Controls**: UAC feature units for playback volume and mute (mapped to
  the DAC's hardware volume and the PA) and capture gain and mute (the ADC
  gain, AGC off); the player's volume keys change the same values and the
  host is told through the interrupt endpoint, so both sides agree.
* **Full duplex**: DAC and ADC run at the same time at the same rate family
  (to be confirmed on the hardware, U-AUD-14); buffers come from a
  "USB audio" partition of the mode pool (02 §4), about 16 KiB, latency
  ≤ 20 ms each way; DMA for both directions (no CPU copies on the audio
  path beyond packet framing).
* **UI**: the USB dialog and Settings › USB offer "Sound card"; its screen
  shows the stream format, input and output level meters, the output route
  (headphones, Bluetooth, both — the PA muted when only Bluetooth is
  wanted), the input source and gain, a monitor (microphone to headphones)
  switch, and works with the screen off; charging continues.
* **Safety** (07 §7): with USB connected the jack must drive headphones or
  equipment that does **not** share ground with the host (direct-drive
  output, virtual-ground return); the manual and a first-use notice say
  so; Bluetooth output has no such limit.
* **Tests**: descriptors and class requests in `orpheus-usb` on the host
  (golden descriptor bytes, every control request, the feedback
  calculator against simulated clock offsets); on the player with the
  workstation's ALSA (`aplay`/`arecord` on the device): enumeration,
  every rate and width, feedback keeping the buffer centred over an hour
  (no xruns, no underruns), playback checked through the Bluetooth A2DP
  capture (null-sink method; the line input may not be used while USB is
  connected), capture checked with a tone from the playback loopback and
  the microphone's noise floor; a `usb-audio` soak scenario (full duplex,
  volume and route changes, host suspend/resume, re-plug); Windows and
  macOS by the owner (user check). M13 measures its audio quality.

## 3. Disk and CD emulation ("USB Drive" app)

Purpose: boot a computer from an `.iso` or `.img` on the card.

### 3.1 Backing store

* The image file's extents are resolved once into the USB pool's extent table
  (8 KiB = about 680 runs). More fragments than that: refuse with the message
  in 05 §7.10. Logical block → card LBA is then a binary search; data moves
  with the same pump as §2.4, bypassing the file system.
* The file system stays mounted read-only for the firmware while an image is
  exported **read-only**. For a writable image the firmware keeps the volume
  mounted read-only too and treats the image's clusters as owned by the host
  (the file's size cannot change through the export).
* Geometry: 512-byte blocks for disks, 2048-byte blocks for CD-ROM; capacity
  from the file size (truncated to a whole block; an ISO whose size is not a
  multiple of 2048 is exported with the tail padded with zeros).
* As built (USB-6): the volume is unmounted while an image is exported (the
  card hand-over of the card reader, §2.4); the 8 KiB table lives in the
  decoder arena after the card reader's transfer buffer. Rules
  (`orpheus_usb::msc::image::check`, CLAUDE.md rule 5): a CD is always
  read-only; a disk is writable only if asked for, the file is under
  `/.orpheus/` and the player is in test mode (developer channel, or the
  developer rows shown); the writable `ImageDevice` reaches only the
  file's own sectors.

### 3.2 Disk mode

Same SCSI set as §2.4 with INQUIRY fields chosen by "Removable disk" (RMB = 1)
or "Fixed disk" (RMB = 0; some firmware menus list only one kind). Hybrid ISO
images boot this way on most UEFI machines and are the simplest path.

### 3.3 CD-ROM mode

Peripheral device type 5, removable. Commands, modelled on what Linux's
mass-storage gadget and common hardware emulators answer (MMC-5):

INQUIRY · TEST UNIT READY (with a "medium may have changed" unit attention
after attach) · REQUEST SENSE · READ CAPACITY · READ (10/12) · READ TOC/PMA/
ATIP (formats 0, 1 and 2 for one data track plus lead-out, MSF and LBA
addressing) · READ HEADER · GET CONFIGURATION (profiles CD-ROM, and DVD-ROM
for images over 700 MiB; features: profile list, core, morphing, removable
medium, random readable, CD read / DVD read) · GET EVENT STATUS NOTIFICATION
(media class) · READ DISC INFORMATION · READ TRACK INFORMATION · MODE SENSE
(10) with the capabilities page 0x2A · MECHANISM STATUS · START STOP UNIT ·
PREVENT ALLOW MEDIUM REMOVAL · SET CD SPEED (accepted) · READ DVD STRUCTURE
(format 0 only, for the DVD profile) · everything else ILLEGAL REQUEST with
correct sense data.

### 3.4 Verification

* Host unit tests: a scripted initiator runs command sequences captured from
  Linux (`sg_raw` traces), Windows and macOS mounts, and firmware boot menus,
  against the class with a file-backed block device.
* **[HIL]** Linux host: `lsscsi` type, `blockdev --getsize64`, full read with
  SHA-256 compare against the file; `isoinfo -d`; for disks a write/read-back
  on a scratch image.
* **[HIL] boot test without a person**: pass the emulated device through to
  QEMU (`-device usb-host,vendorid=…,productid=…`) and boot a small ISO under
  both SeaBIOS and OVMF with the serial console captured; pass = the image's
  boot banner appears. Needs `qemu-system-x86` on the host and the udev rule;
  if unavailable, the test is reported as not run and a user check is added
  (boot a real PC from two named ISOs: a Linux hybrid ISO and a Windows
  installer ISO).

## 4. Bluetooth

### 4.1 What is on the board

An autonomous Bluetooth audio **transmitter** module (answers an identity
query with "ATS2851") on UART1 (`0xC00E0000`, 115200 8N1, GPIO38/39), enabled
by GPIO20, with GPIO31 as a host-busy line. It runs the whole Bluetooth stack
and the audio encoder itself. Audio reaches it as an **analogue** signal from
the SoC's DAC/headphone output (inferred by elimination: the UART carries no
audio and the I2S pins are used for other things). The player speaks a small
proprietary command protocol (`AA`-framed, about 25 commands and 20 events,
documented byte for byte in docs/hw/bluetooth.md §3–4).

### 4.2 In scope: transmitter role

Crate `orpheus-bt` (protocol, host-testable) + `yoton-bsp::bt` (UART, pins).

* **Link**: UART1 with RX by DMA (channel 8) into a ring polled every
  10 ms, TX by DMA (channel 9, mode `0x00228200`). As built (BT-2 device):
  single-byte transfers with reload (`0x00260082`) into a 256-byte ring,
  the write position read from the channel's remain counter (U-BT-4), so
  the ring is never stopped and GPIO31 stays low; a CPU-polled FIFO path
  (1 ms) is the fallback. The vendor instead ran burst-8 into a 128-byte
  pattern-filled buffer, stopped it to drain it, and raised GPIO31
  meanwhile.
* **Codec**: frame parser/encoder with checksum, resynchronisation on `AA`,
  length limits; fuzzed.
* **Command layer**: `async fn command(id, params) -> Result<Ack>`: one frame
  out, matching event 0x00 in within 500 ms, one retry, then the link is
  declared down and the module power-cycled through GPIO20.
* **State machine**: Off → Starting (identity, settings) → Idle → Inquiry →
  Connecting → Connected → Streaming; paired-device list from the module;
  auto-reconnect to the last device with the stock configuration's limits (a
  bounded number of attempts at power-on and on mode entry).
* **Audio**: nothing changes in the pipeline. The DAC plays as for headphones;
  the module is told "audio started/stopped" (command 0x05). Volume while
  streaming: keep the DAC at a fixed reference level and send absolute volume
  to the sink (command 0x26), falling back to local volume for sinks without
  absolute volume. **As built (2026-11-24):** the module's
  acknowledgement and volume events cannot tell whether a sink scales (it
  echoes the sent value; a sink without absolute volume answers 0x25 with
  128), so the DAC always carries the player's volume and the sink is sent
  full scale. Find the reference level by test (highest level without
  clipping at the module's input). *As built (UI-6a): the player's volume
  goes to the sink as 0x26 at every connection and on every change (a
  BlueZ sink starts at 0), sink changes (event 0x26) become the player's
  volume; once the module acknowledges 0x26 (or the sink reports a volume)
  the DAC is held at −3 dB (the tap is after the DAC volume, U-BT-2), back
  to the player's volume on disconnect or when the sink says it has no
  absolute volume.* Whether to mute the wired headphone output
  while streaming depends on where the tap is (bluetooth.md Q2); make it a
  setting until known.
* **Remote control**: headset keys (event 0x01: play, pause, stop, next,
  previous) go to the playback service; sink volume changes (0x25/0x26) update
  the volume state; track title/artist/album are sent with command 0x31 at
  each track change.
* **Radio and recorder monitor over Bluetooth** work for free if the module's
  tap is after the headphone mixer (FM is an analogue bypass into that mixer);
  to be observed, not designed.
* **Power**: GPIO20 low whenever Bluetooth is off; module shutdown command
  (0x24) before that.
* **Do not send the upgrade commands (0x20–0x23), the baud change (0x28) or
  the settings write (0x2C/0x7F)** except in the work package that owns them,
  and never the upgrade without a known-good image and the user's explicit
  go-ahead: recovery from a failed module upgrade is unknown.

Acceptance: host tests replay the start-up exchange preserved in the RAM dump
and the sequences of bluetooth.md §4; **[HIL]** identity query answered;
inquiry lists a nearby device; user check: pair a headset, hear music, keys
and volume work, reconnect after power cycle.

First hardware step (blocks everything else): which of GPIO38/39 is TX
(bluetooth.md Q3). Test without a logic analyser: enable the module, configure
one pin as UART TX and the other as RX, send the identity command, look for
the reply; then swap.

### 4.3 Presenter remote and tilt pointer over Bluetooth HID

Owner's requirement (2026-11-24, reversing the removal of 2026-11-23): the
player becomes a **Bluetooth presentation remote with a tilt-controlled
pointer** (the "IMU mouse"). The app is specified in 05 §7.12; this section
covers the transport and the pointer engine.

**Evidence so far.** The module's command set has no documented HID command,
but its SDP record, read by a BlueZ host on 2026-11-24, lists **Human
Interface Device (0x1124)** next to A2DP, AVRCP, HFP and PnP (01-hardware
§9). So the module firmware contains a HID service; whether the player can
feed it reports is the open question.

#### 4.3.1 Transport: staged with stop/go points (WP BT-3, then BT-4)

**Step 1 done (2026-11-24, lead)**: the module's SDP HID record, read from
the workstation with a raw SDP query: subclass `0xC0` (combo
keyboard/pointing), country 33, virtual cable and normally-connectable set,
supervision timeout 5000, 83-byte report descriptor with **report 1 = boot
keyboard** (8 modifiers, LED output, 3 key slots over usages 0–255 — every
presenter key: arrows, PageUp/PageDown, F5, Esc, B, W, Ctrl+L) and **report
2 = consumer control** (Vol−, Vol+, Mute, Play/Pause, Scan next, Scan
previous, 0x83 Recall last, Rewind). **No mouse report.** Consequence: the
remote's keys fit the module as it is, if step 2 finds the command that
feeds these reports; the **tilt pointer cannot be carried by this
descriptor**; per D-10 (owner, 2026-11-24) it is built only if another
Bluetooth path for mouse data is found, and dropped otherwise.

1. **Read the HID record** (no risk, host side): with the module paired to
   the workstation, read its SDP HID attributes (report descriptor, subclass,
   `HIDVirtualCable`, `HIDReconnectInitiate`, boot device flag) through
   BlueZ D-Bus/`sdptool`, and let BlueZ's input plugin connect the HID
   control and interrupt channels (L2CAP PSM 0x11/0x13). Record which report
   ids exist (keyboard, consumer, mouse?) and whether the connection is
   accepted. Watch the module's UART for new events while the host connects
   and sends output reports (LEDs).
2. **Find the feeding command**: the unexplained commands (0x27, 0x2D, 0x30,
   0x32, sub-commands 0x16/0x17 of 0x05) are first candidates; then the
   vendor firmware (BTENGINE/ the remote-control or "selfie" code paths, HID
   strings) for a frame that carries key or report bytes; then a scan of the
   command-id space **excluding** 0x20–0x24, 0x28, 0x2C and 0x7F (CLAUDE.md
   rule 6) with the host connected as HID host, logging every report the
   host receives (`hidraw`). Stop/go: a command that makes the host receive
   a report.
3. **Map the capability**: which keys/usages and whether relative mouse
   reports are reachable; report rate and latency (host timestamps vs the
   player's send time; target ≤ 30 ms, ≥ 60 reports/s for the pointer);
   coexistence with A2DP (both at once, or one role at a time).
4. **Owner's decision D-10 (2026-11-24)**: the remote is a Bluetooth remote
   (keyboard + consumer reports). **Mouse data**: look for any Bluetooth
   path that carries it (another descriptor or record the module can
   switch to, a command that sends pointer reports, the module's own
   remote/"selfie" modes). If none exists, the tilt pointer (§4.3.2) is
   **dropped** — no USB pointer and no module firmware work for it.

**Built in every case** (host-testable, transport-independent):
`orpheus-hid` — report descriptors (boot keyboard, consumer control,
relative mouse with wheel, optional absolute pointer), report builders, key
press/release sequencing with auto-release on link loss, and a transport
trait with three implementations: Bluetooth module (BT-4), USB (USB-5), and
the simulator/WASM back end that renders what the computer would see.

**Step 1 continued (2026-11-24, lead)**: with the module connected for
audio, BlueZ's `ConnectProfile(0x1124)` **succeeds**: the host gets a HID
keyboard `ZS285AH_TARNS` (`/dev/hidraw*`, HID id `0005:10D6:B009`, input
device "ZS285AH_TARNS Keyboard"). The HID channel works alongside A2DP;
only the command that makes the module send reports is unknown (step 2).
The project udev rule grants raw access to it and tells libinput to ignore
it, so probes never type into the desktop.

**Step 2, first pass (2026-11-24)**: `0x27` answers event `27 00`; `0x2D`,
`0x32 00 01`, `0x05 16/17` are acknowledged; **`0x32 01 00` restarts the
module** (event `0x23`, full start-up, reconnect). Next: with raw HID
access, watch for reports after each candidate, and look in the vendor
firmware for the senders of 0x32 and the meaning of its two bytes before
probing more values.

**Step 2, second pass (2026-11-24)**: with raw HID access, none of `27`,
`2D` (with or without a parameter) or `05 16/17` produced a HID report;
**`0x32` restarts the module with any value** and drops the HID link. The
live probing of the documented candidates is exhausted; the next step is the
offline analysis of the vendor host firmware (engine message 0x332c → 0x32,
and any report-carrying path), before a wider command-id scan.

#### 4.3.2 Tilt pointer engine (WP SENS-2) — conditional (D-10)

Built **only if** §4.3.1 finds a Bluetooth transport for mouse data; if not,
this section and SENS-2 are dropped and the remote ships without a pointer.

The sensor is a 3-axis accelerometer without gyroscope (MiraMEMS DA2xx at
I2C `0x27`), so the pointer is driven by **tilt** (the gravity vector), not
rotation rate. Crate `orpheus-pointer` (host-testable, `no_std`, fixed
memory):

* **Sampling**: 100 Hz over I2C (the sensor's FIFO if it has one, else a
  timer-driven 6-byte read, ≈ 0.7 ms at 100 kHz), only while the pointer is
  active; the step counter's low rate otherwise.
* **Orientation**: pitch and roll from the gravity vector, relative to the
  **neutral pose captured when the clutch is pressed**, so the user can hold
  the player any way; corrected for the screen-rotation setting.
* **Modes**:
  * *Relative (joystick)*: angular offset beyond a dead zone (default 3°)
    maps to cursor velocity through a curve `v = g · (|θ| − dz)^1.6`, capped;
    sub-pixel accumulation; one-euro filter against hand tremor and sensor
    noise.
  * *Absolute (laser)*: tilt angle maps to a screen position (±20° covers the
    screen by default) through the absolute-pointer report; returns to the
    centre at clutch press.
  * *Scroll*: tilt drives the wheel (vertical and horizontal).
* **Clutch**: the pointer moves only while the clutch key is held (default),
  or toggles with a tap (setting); release freezes the cursor — this removes
  drift and accidental motion, the main weakness of tilt pointers.
* **Linear acceleration** (hand jerks) is rejected by ignoring samples whose
  magnitude departs from 1 g by more than a threshold.
* **Settings**: speed, dead zone, smoothing, invert axes, mode, clutch
  behaviour; a calibration screen shows the live tilt and the dead zone.
* **Tests**: recorded sensor traces (SENS-1's capture tool) replayed on the
  host with expected cursor paths (golden CSV); properties: no motion within
  the dead zone at rest, zero drift over 60 s of rest data, symmetric
  response, bounded latency (≤ 2 samples); fuzzed sample streams; on the
  device, traces are injected through a test command (a fake sensor source)
  so HIL tests need no hand movement; the feel is a user check.

#### 4.3.3 Acceptance

**[HIL]** with the workstation as HID host (BlueZ, `hidraw` read through the
project udev rule): every remote action produces the expected report
sequence for each profile; injected sensor traces produce the expected mouse
reports; reconnect after power cycle; link loss releases all keys.
**User checks**: a real presentation in PowerPoint or LibreOffice Impress and
Google Slides (next/previous/black/end, timer), pointer feel in each mode,
battery use over an hour of presenting.

## 5. Authenticator

### 5.1 Function

HOTP (RFC 4226) and TOTP (RFC 6238): SHA-1, SHA-256, SHA-512; 6 or 8 digits;
30 or 60 s periods; up to 32 entries. Crate `orpheus-otp` on RustCrypto `hmac`,
`sha1`, `sha2` (all `no_std`, allocation-free).

### 5.2 Storage and what it does and does not protect against

* Entries live in the NOR vault sector(s): a header (version, KDF salt,
  iteration count, verifier) and encrypted records
  (XChaCha20-Poly1305, one nonce per record write).
* The key is derived from the PIN or passphrase with PBKDF2-HMAC-SHA-256, the
  iteration count calibrated at set-up to about one second at the highest
  clock level, mixed with the device id from the eFuse words.
* **Honest limit**: this SoC's boot ROM lets anyone with the player and a USB
  cable read the whole NOR and the eFuse words. A 4–8 digit PIN therefore only
  stops casual access through the UI; it does not survive an offline
  brute-force attack. A long passphrase does. The set-up screen says so in one
  sentence and offers both; the default is a 6-digit PIN with the warning.
  There is no secure element; do not describe the feature as hardware-backed.
* Wrong-PIN delays (1 s doubling to 5 min) are enforced from a counter stored
  in the vault header before each attempt.
* The key and decrypted secrets exist in RAM only while the app is unlocked
  and are zeroised on lock, screen-off and before any reboot into ADFU
  (the RAM survives resets).

### 5.3 Time

TOTP needs UTC within a few seconds. Sources: the developer channel's TIME
request from a host utility (`xtask time-sync`, also built as a standalone
`orpheus-time` binary); RDS clock time if the tuner provides it; manual setting.
The RTC's drift is measured between two synchronisations and stored as a
correction (seconds per day). The app shows when the clock was last set and
warns if the RTC reports having stopped (docs/hw/system.md T6 decides how that
is detected; until then, a "clock set" marker in the settings store compared
with an implausible date).

*As built (VER-13):* the RTC keeps local time (what every screen shows);
UTC for the codes is that less Settings › Date & time › Time zone
(`services::utc_of_local`, firmware `vault::unix_now`). Correction (s/day,
set by hand; not yet measured between synchronisations) steps the running
clock a whole second at a time (`services::ClockDrift`). Set from RDS takes
the clock time of group 4A once the next minute's group agrees with it
(`orpheus_fm::rds::Rds::clock`; the C and D blocks carry no error report)
and sets the RTC in the player's time zone. Neither moves the clock during a
test session.

### 5.4 Provisioning and output

* Import: `otpauth://` URIs, one per line, from `/.orpheus/otp-import.txt`
  (inside `/.orpheus/`, CLAUDE.md rule 5; OTP-2); the file is overwritten
  with zeros and deleted after an import that took every line (a file with
  lines that were not understood is kept); the UI notes that flash cards may
  keep old copies. Or the PROVISION request of the developer channel
  (`cargo xtask dev otp-add 'otpauth://…'`; only while the vault is
  unlocked, added after an on-screen confirmation). Or manual entry with the
  Base32 keyboard (not built yet).
* Output: on screen; typed over the USB keyboard interface (boot-protocol
  keyboard, so it also works in firmware set-up screens and full-disk-
  encryption prompts), digits as top-row or keypad usages (setting, for
  layouts where the top row needs Shift), optional Enter. Built in USB-5 /
  OTP-2 (2026-11-24): the keyboard personality (developer channel + HID boot
  keyboard, 1 interrupt IN endpoint, 8-byte reports, bInterval 4), the
  computer's layout as a setting (US, UK, German, French; characters behind
  dead keys are refused before anything is typed), a press and an
  all-released report per character 12 ms apart, all keys released on
  suspend, reset and personality change. `orpheus-hid` holds descriptors,
  reports, layouts and the typist for the Bluetooth transport too (BT-4).
* RAM (OTP-2): while unlocked the firmware keeps only the 32-byte key; the
  tag is checked at unlock by streaming the sector through Poly1305, records
  are decrypted one at a time on demand, and changes borrow 3.8 KiB of the
  frame buffer for the moment of the write (wiped after, screen redrawn).
  The key derivation runs to the end in one UI step (≈ 0.3 s at 144 MHz).
  Device id for the KDF: a fixed label until the eFuse words can be read
  without PMU writes (§5.2; changing it later needs a new vault).

### 5.5 Later, optional: FIDO U2F / CTAP2

A FIDO authenticator over USB HID (one interrupt IN and one OUT endpoint,
P-256 with the `p256` crate) fits the hardware, but shares the at-rest limit of
§5.2 and needs careful review. It is listed as an optional package after the
release, not part of the committed scope.

## 6. Work packages

* **WP-USB-1 Controller driver**: bring-up, enumeration with a vendor-only
  device, developer channel 0x01/0x02/0x07; settle U-USB-1, -2, -3, -6, -7, -9.
* **WP-USB-2 Developer channel complete** + host library + `xtask dev` live
  commands (log, shot, keys, stats, test, time).
* **WP-USB-3 Mass storage**: class, pump, performance, integrity suite.
* **WP-USB-4 Personality manager, USB dialog, charge-only and charger
  detection** (with the `power` task).
* **WP-USB-5 HID keyboard**: interface, authenticator typing; U-USB-11.
* **WP-USB-6 Disk and CD emulation** + QEMU boot test.
* **WP-USB-7 USB audio**: superseded by milestone M16 (§2.7).
* **WP-BT-1 Link and protocol**: UART1 driver, pin test, codec, command layer,
  identity/settings; host replay tests.
* **WP-BT-2 Transmitter role**: state machine, pairing UI, AVRCP bridge,
  volume policy, auto-reconnect, power handling.
* **WP-BT-3 HID investigation** (§4.3.1 steps 1–3), report in
  docs/hw/bluetooth.md; stop/go for BT-4.
* **WP-BT-4 HID transport**: `orpheus-hid` (descriptors, reports, transport
  trait, simulator back end) and the module transport.
* **WP-SENS-2 Tilt pointer** (conditional, D-10: only with a Bluetooth
  transport for mouse data): `orpheus-pointer`, sensor fast-sampling mode,
  trace tests, fake-sensor test command.
* **WP-OTP-1 `orpheus-otp`, vault, PIN flow, time sync**; **WP-OTP-2 typing and
  provisioning.**
