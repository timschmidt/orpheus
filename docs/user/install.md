# Install, update and restore

## Update

No release has been published yet: the update file
(`orpheus-<version>.ofw`) comes from building the firmware
(`cargo xtask build`). There are three ways to install it; a power cut
during any of them is harmless, because the player starts either the
previous version or the new, complete one.

**From the project's tools** (works; on the reference player an update
takes about 40 s):

```sh
cargo xtask build                                   # also writes orpheus-<version>.ofw
cargo xtask dev update target/orpheus/orpheus-*.ofw
```

**On the player** (**in progress**: built, not yet tried on the player).
Copy the `.ofw` file to the top folder of the card (or
`/.orpheus/update/`), then *Settings › System › Update firmware*: choose
the file, read its changes, *Install*. The player checks the whole file,
writes it next to the running firmware, reads it back, switches and
restarts. Keep 30 % battery or the cable in; music, settings and the card
stay.

**From the browser** (**in progress**: tried against a simulated player,
not yet against a real one). Open the site's
[Install page](https://timschmidt.github.io/orpheus/install.html) in
Chrome, Edge, Opera or Brave, *Choose the player*, choose the `.ofw`
file, *Update Orpheus*. No driver is needed.

## If the new version does not start

After three failed starts the player starts the previous version by
itself (Settings › System › About then shows *Failed to start*; this was
tested on the reference player with a firmware made to crash). If that
fails too, it goes to **recovery mode** (its chip's USB recovery, which
nothing can overwrite). On the Install page, *Choose the player* again,
then *Recovery › Start the other firmware*. With the tools: `cargo xtask
dev switch-slot`, then `cargo xtask dev reset`.

## First install

On a player with the original firmware the Install page is meant to do
it in recovery mode (**in progress**: not yet tried on a real player):

1. *Back up the whole flash*: read twice and compared, kept in the browser
   and downloaded as a file. Keep that file.
2. Confirm that the next step rewrites the boot record, then *First
   install of Orpheus*. If it is interrupted, the page offers to continue.

The original firmware has no button for recovery mode: the first time,
`cargo xtask dev adfu` (Linux) switches it. On **Windows**, recovery mode
needs the WinUSB driver once (for example with Zadig, device
`10D6:10D6`).

The command-line way, which is how the reference player was set up:
`cargo xtask dev nordump 0 0x800000 local/nor-before-full.bin`,
`cargo xtask dev backup-verify`, `cargo xtask dev install-stage2`,
`cargo xtask dev install-full --i-have-a-backup`. The tools still need the
companion repository
[yoton-rockbox](https://github.com/timschmidt/yoton-rockbox) for two small
helper programs (point `ORPHEUS_DIR` at it) and the udev rule:

```sh
sudo cp tools/udev/99-orpheus.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

## Restore the original firmware

On the Install page in recovery mode: choose the backup (the one the page
kept, or the downloaded file), confirm, *Restore the original firmware*.
Every part of the flash that differs from the backup is written back, the
boot record last but one; the data the original firmware keeps at the end
of the chip is never touched. With the tools:

```sh
cargo xtask dev restore vendor --verify               # compare only
cargo xtask dev restore vendor --i-have-a-backup      # write it back
```

## How the player is laid out

Orpheus uses the flash from `0x001000` to `0x7DFFFF`: two firmware slots
(an update goes into the one not running), its settings and a 1 KiB boot
stage. The first 4 KiB (the boot record) are written only by the
confirmed steps above, never by an update; the last 128 KiB (data the
original firmware left) are never written. See [troubleshooting](troubleshooting.md).
