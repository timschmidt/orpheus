# Add a test command

The running firmware accepts short text commands over the developer
channel (a vendor request on USB endpoint 0, available in every USB
personality). They drive hardware-in-the-loop tests without touching the
keys:

```sh
cargo xtask dev cmd 'status'               # send, wait, print the reply
cargo xtask dev cmd 'playdir /Music/x' --wait 5
```

1. **Find the dispatcher.** `fw/firmware/src/usbdev.rs`,
   `test_command(&mut self, data: &[u8]) -> bool`. Commands are matched on
   their bytes (`data == b"gov on"`, `data.strip_prefix(b"bt ")`); return
   `true` if the command was taken.
2. **Pick where it runs.** The dispatcher runs on the main executor:
   only flip a flag, signal a task or queue a request there.
   * Subsystem families have their own parser:
     `bt::test_command`, `drain::test_command`.
   * Card and playback commands go to the audio task's queue
     (`audio::CMDS`), which serves them between tracks.
3. **Reply.** Write the reply text into the shared reply buffer (see
   `audio::status_reply`); `xtask dev cmd` reads it after `--wait`
   seconds. Keep replies short and machine-readable (`key=value`), since
   HIL tests parse them.
4. **No `core::fmt`.** Format numbers with the firmware's own helpers; a
   linked `core::fmt::write` fails `xtask size --check`.
5. **Safety.** Commands that write must stay inside `/.orpheus/` on the
   card and inside the NOR regions the firmware owns. Commands that could
   be harmful (Bluetooth module ids 0x20–0x24, 0x28, 0x2C, 0x7F) are
   refused by the existing filters; keep it that way.
6. **Document it** in the work package's STATUS entry and, if a test
   uses it, in the HIL suite (`xtask/src/dev/hil.rs`).

Test-only behaviour that changes what the user sees (for example
switching power-off into a reboot) runs only in test mode, which the
harness arms through the persistent mailbox.
