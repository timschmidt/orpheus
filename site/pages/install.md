# Install on your YM03

> **Browser installer: coming.** This page will install, update and
> restore Orpheus from Chrome, Edge and other Chromium-based browsers: it
> first saves a backup of the player's whole flash chip to your computer,
> checks every image before writing, and never touches the parts of the
> chip it does not own. It is work package WEB-3 and is not built yet;
> Firefox and Safari cannot talk to USB devices and will keep the
> command-line path.

## Today: the command line

Installing works from a Linux terminal with the project's tools, and they
are still set up for the project's reference player. The manual has the
whole procedure, including the backup and the way back:

* [Install, update and restore](manual/install.html)
* [Troubleshooting and recovery](manual/troubleshooting.html): recovery
  mode, the boot counter, restoring the original firmware

Updating an installed Orpheus is two commands:

```sh
cargo xtask build
cargo xtask dev stage target/orpheus/firmware.yfw
```

## Is it safe?

The YM03's USB recovery mode is in read-only memory: whatever is in the
flash chip, the player can always be reached over USB and the original
firmware put back from the backup. Orpheus never writes the chip's boot
record or the data the original firmware keeps at its end during normal
use. See the [FAQ](manual/faq.html).
