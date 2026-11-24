# Install, update and restore

> **Read this first.** Today Orpheus is installed with command-line tools
> on a Linux computer, and the tools are still set up for the project's
> reference player. A browser installer (Chrome, Edge and other
> Chromium-based browsers) that makes a backup first and needs no terminal
> is **in progress**. If you are not comfortable with a terminal and with
> reading the [safety rules](../dev/getting-started.md#8-working-with-the-player-safely),
> wait for it.

## What you need

* A Yoton YM03, a USB cable, a Linux computer.
* The Orpheus repository and its toolchain:
  [getting started](../dev/getting-started.md) (about 15 minutes).
* The companion repository
  [yoton-rockbox](https://github.com/timschmidt/yoton-rockbox): the tools
  still take two small helper programs that run on the player (the USB
  monitor `ymon` and the flash reader) from it, and they look for the
  backup of the original firmware in its `dumps/` folder. Point
  `ORPHEUS_DIR` at your copy. `backup-verify` currently accepts only the
  reference player's backup (it checks a fixed checksum).
* The udev rule, so the tools may talk to the player without root:

  ```sh
  sudo cp tools/udev/99-orpheus.rules /etc/udev/rules.d/
  sudo udevadm control --reload && sudo udevadm trigger
  ```

## How the player is laid out

The player keeps its software in an 8 MiB flash chip. Orpheus uses
everything from `0x001000` to `0x7DFFFF`: the firmware, its settings and a
1 KiB boot stage. The first 4 KiB (the chip's boot record) and the last
128 KiB (data the original firmware left) are never written by the normal
commands. The chip's built-in USB recovery mode (ADFU) cannot be
overwritten, so the player can always be reached over USB.

## First install

**In progress.** On a player that still has the original firmware, the
first install needs the boot record of the Rockbox port, which Orpheus
reuses; today it is put there with the companion repository's tools. A
one-command first install (`xtask dev install-boot`) and the browser
installer will replace that. On the reference player the sequence was:

1. Back up the whole flash chip and check the backup:
   `cargo xtask dev nordump 0 0x800000 local/nor-before-full.bin`, then
   `cargo xtask dev backup-verify`.
2. `cargo xtask dev install-stage2`: the Orpheus boot stage.
3. `cargo xtask dev install-full --i-have-a-backup`: Orpheus over the
   original firmware.

## Update

With Orpheus running and the player plugged in:

```sh
cargo xtask build                                     # builds target/orpheus/firmware.yfw
cargo xtask dev stage target/orpheus/firmware.yfw     # writes it, with verify
```

`stage` checks the backup, switches the player to recovery mode and writes
the firmware with a read-back check; `cargo xtask dev reset` then starts
the new version. Settings and everything on the card stay.

## Restore the original firmware

```sh
cargo xtask dev restore vendor --verify               # compare only
cargo xtask dev restore vendor --i-have-a-backup      # write it back
```

This writes the saved copy of the flash back to the whole chip: the
player is then exactly as it was when the backup was made, with the
original firmware and its settings.

## If an install is interrupted

Nothing is lost: plug the player in again and repeat the command. If the
player does not start, see [troubleshooting](troubleshooting.md): recovery
mode is always reachable.
