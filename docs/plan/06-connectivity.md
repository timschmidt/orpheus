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
| USB audio device | **Probably** (stretch) | Needs isochronous FIFOs with the Actions DMA; untested (U-USB-12) |
| USB host / OTG | **No (not planned)** | The core is a dual-role Faraday design, but nothing in the vendor image uses host mode and there is no sign of a VBUS supply |
| Bluetooth: send music to headphones/speakers | **Yes, cheap** | The board carries an autonomous transmitter module (self-reports "ATS2851") that runs its own stack and takes **analogue** audio from the DAC; the player only sends short UART commands |
| Bluetooth: remote keys and volume from the headset; volume and track text to the headset | **Yes** | In the module's command set |
| Bluetooth: receive audio and play it through the DAC | **Out of scope** (owner's decision, 2026-11-23) | The module's stock firmware has no such command and no audio route from the module back into the SoC is known |
| Bluetooth: HID (presentation remote, code typing), BLE | **No with the module's stock firmware** | No such command (§4.3) |
| One-time codes (TOTP/HOTP) on screen | **Yes** | Needs a correct clock |
| Typing codes into a computer | **Yes over USB HID**; over Bluetooth only if §4.3 succeeds | |
| Presentation / media remote over USB | **Out of scope** (owner's decision, 2026-11-23) | Possible, but a remote that needs a cable is unattractive |
| Mouse / pointer control | **Out of scope** (owner's decision, 2026-11-23) | The sensor is a 3-axis accelerometer (MiraMEMS DA2xx family) with no gyroscope, so only a tilt mouse would have been possible |

The Bluetooth HID "No" is a limit of the fitted Bluetooth module, not of this
plan. §4.3 describes the investigation that could lift it and what would
have to be true. Nothing else in the plan depends on it. The "Out of scope"
rows are not to be implemented or investigated.

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
| `Bus::poll` | Awaits a `Signal` set by the IRQ 12 handler: reset (re-program endpoint/FIFO maps, as `bus_reset()` in the port), suspend, resume; power events from the `power` task (VBUS) |
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
| USB audio (stretch) | UAC1 speaker (+ HID consumer control) | iso OUT, iso feedback IN, interrupt IN | Player becomes a USB DAC |
| Recovery | — | — | "Recovery mode" reboots into the ROM's ADFU |

Every personality also answers the **developer channel** on endpoint 0 (§2.5),
so tools work regardless of what the player is presenting.

Identity: the Rockbox port borrows Actions' vendor id (`10D6:1157`). yotonrs
uses the pid.codes test id `1209:0001` during development and a product id
requested from pid.codes for release (decision D-3 in PLAN.md). Strings:
manufacturer "yotonrs", product per personality, serial from the eFuse-derived
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
Medium removal mid-transfer returns NOT READY / MEDIUM NOT PRESENT and never
hangs the bus. "Read-only card reader" setting reports write-protect in MODE
SENSE and rejects writes.

### 2.5 Developer channel (endpoint 0, vendor requests)

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

Crate `yoton-bt` (protocol, host-testable) + `yoton-bsp::bt` (UART, pins).

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
  absolute volume. Find the reference level by test (highest level without
  clipping at the module's input). Whether to mute the wired headphone output
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

### 4.3 Investigation track: can the module do HID? (not committed)

The user would like the player to act as a Bluetooth presentation remote. That
needs a HID role, which the module's command set does not offer. No remote is
built in this plan (a USB-tethered one was removed from scope on 2026-11-23,
with the receiver role and the mouse); this track only answers whether a
Bluetooth one is possible. It is a sequence of cheap checks with a stop/go
after each; none of it blocks the rest of the plan.

1. **Observe** (no risk): with the stock firmware in Bluetooth mode, scan from
   a phone. Is the module discoverable/connectable? What name, address and
   class of device does it show? (bluetooth.md Q8)
2. **Probe the unexplained commands** (0x27, 0x2D, 0x30, 0x32, sub-commands
   0x16/0x17 of 0x05, the three fixed inquiry bytes) and scan the command-id
   space for acknowledgements, **excluding** 0x20–0x24, 0x28, 0x2C and 0x7F.
   Log every event. A "role" or "mode" command would show up here.
3. **Module firmware**: is an SDK, tool chain or alternative image with a HID
   role obtainable for this chip (docs/research/rust-ecosystem.md §9–10)? Can
   the stock image be read back? The chip marking would help identify it
   (needs the case opened: decision D-5, optional). Only if such firmware
   exists is a HID role a project; it would be planned separately, with
   module-bricking risk stated to the user first (decision D-6).

The result is a short report. If a HID role exists, a Bluetooth remote and
Bluetooth code typing go to the owner as a new decision; nothing is built
without it. If not, the Bluetooth app keeps the transmitter role only and
authenticator typing stays on USB.

## 5. Authenticator

### 5.1 Function

HOTP (RFC 4226) and TOTP (RFC 6238): SHA-1, SHA-256, SHA-512; 6 or 8 digits;
30 or 60 s periods; up to 32 entries. Crate `yoton-otp` on RustCrypto `hmac`,
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
`yoton-time` binary); RDS clock time if the tuner provides it; manual setting.
The RTC's drift is measured between two synchronisations and stored as a
correction (seconds per day). The app shows when the clock was last set and
warns if the RTC reports having stopped (docs/hw/system.md T6 decides how that
is detected; until then, a "clock set" marker in the settings store compared
with an implausible date).

### 5.4 Provisioning and output

* Import: `otpauth://` URIs, one per line, from `/otpauth.txt`; the file is
  overwritten with zeros and deleted after a successful import (note in the UI
  that flash cards may keep old copies). Or the PROVISION request of the
  developer channel. Or manual entry with the Base32 keyboard.
* Output: on screen; typed over the USB keyboard interface (boot-protocol
  keyboard, so it also works in firmware set-up screens and full-disk-
  encryption prompts), digits as top-row or keypad usages (setting, for
  layouts where the top row needs Shift), optional Enter.

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
* **WP-USB-7 USB audio** (stretch; U-USB-12).
* **WP-BT-1 Link and protocol**: UART1 driver, pin test, codec, command layer,
  identity/settings; host replay tests.
* **WP-BT-2 Transmitter role**: state machine, pairing UI, AVRCP bridge,
  volume policy, auto-reconnect, power handling.
* **WP-BT-3 Investigation track** (§4.3), report in docs/hw/bluetooth.md.
* **WP-OTP-1 `yoton-otp`, vault, PIN flow, time sync**; **WP-OTP-2 typing and
  provisioning.**
