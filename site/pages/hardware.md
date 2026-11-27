# Hardware

The Yoton YM03 is a small clip-on MP3 player for sport. This page says
what is inside it and what Orpheus does with each part. The detailed
reference is [01-hardware.md](https://github.com/timschmidt/orpheus/blob/main/docs/plan/01-hardware.md)
in the plan.

## The YM03 and what Orpheus uses

| Part | What it is | Orpheus |
|---|---|---|
| SoC | Actions ATJ2157: ARM Cortex-M4F with FPU | Runs at 24–288 MHz; 1.0 V core supply up to 144 MHz instead of the default 1.2 V; slows down when idle |
| RAM | 320 KiB in four blocks | Resident code, one frame buffer, page caches, audio rings ([architecture](dev/architecture.html)) |
| Flash | 8 MiB SPI NOR, not memory-mapped | Firmware, settings, boot stage; code is paged into RAM 1 KiB at a time |
| Screen | 128 × 128 colour LCD, 16-bit colour, backlight | Every pixel drawn by Orpheus, updated by DMA |
| Keys | Ten: Up, Down, Prev, Next, OK, Menu, Back, Vol+, Vol−, Power | One meaning per key on every screen |
| Card | microSD, FAT32 or exFAT | Music, pictures, books, the library index; writes only in `/.orpheus/` and the Recorder's `/Recordings` |
| Audio out | DAC with hardware volume and a headphone amplifier | Music fed by DMA; volume in hardware |
| Bluetooth | A transmitter module on a serial link, fed with the analogue audio | Pairing, connecting, sending audio; a presenter remote is being investigated |
| USB | USB 2.0 device | Card reader, developer channel, disk and CD images for a computer, a keyboard that types one-time codes, firmware updates |
| FM tuner | RDA5807-family | Radio with presets, scanning and RDS; the audio takes the analogue path past the processor |
| Motion sensor | 3-axis accelerometer (no gyroscope) | Step counter, sampled at 25 Hz |
| Microphone | Analogue input | Recorder (microphone and radio) |
| Clock | Real-time clock with alarm | Clock on the status bar and lock screen, alarms while the player is on |
| Battery | Lithium cell, charger in the power-management unit | Charged at the original firmware's current (200 mA code); level and charge state shown |

## What makes it interesting

* **No execute-in-place.** The flash is not mapped into the address
  space, so firmware larger than the RAM runs through a software demand
  pager built on the memory protection unit.
* **Hardware the original firmware leaves unused**: an FFT/filter engine,
  a hardware I2C controller, deeper standby states. Orpheus investigates
  each and uses it where it helps.
* **Recovery cannot be lost.** The chip's USB recovery mode is in ROM, so
  development is safe.

## Not published

Notes derived from the original firmware and the vendor's documents stay
out of the repository (decision D-7 in the plan): this page describes the
hardware only as far as Orpheus's own documents do.
