# Install on your YM03

This page updates, installs and restores Orpheus from Chrome, Edge, Opera,
Brave or another Chromium-based browser, over USB. It uses the same code
as the project's command-line tools. Before it writes anything in recovery
mode it reads the player's whole flash chip twice, keeps the copy in this
browser and gives it to you as a file; it checks every file against the
release's checksums; and it never writes the chip's boot record except in
the steps you confirm, nor the data the original firmware keeps at its
end. Not sure? [Try the installer on a simulated player](install.html?demo)
first.

<link rel="stylesheet" href="install/install.css">
<div class="inst-note" id="inst-unsupported" hidden>
<p><b>This browser cannot talk to USB devices.</b> Firefox and Safari do
not support WebUSB. Use a Chromium-based browser on a computer, or the
command line below.</p>
</div>
<div class="inst" id="install" data-mode="none" hidden>
<p class="inst-note" id="inst-demo" hidden><b>Demo:</b> a simulated player,
nothing real is written. <a href="install.html?demo=stock">Start from the
original firmware</a> · <a href="install.html?demo">start from Orpheus</a>.</p>
<p id="inst-release"></p>
<div class="inst-note" id="inst-resume" hidden><p id="inst-resume-text"></p><p><button type="button" id="inst-resume-go">Continue</button></p></div>
<p><button type="button" id="inst-connect">Choose the player</button> <span id="inst-device">No player chosen.</span></p>
<p class="inst-status" id="inst-status" role="status" aria-live="polite"></p>
<p><progress id="inst-progress" hidden></progress> <span id="inst-progress-text"></span></p>
<section id="inst-orpheus" hidden aria-label="Orpheus is running">
<h2>Update</h2>
<p>Writes the new version into the player's second firmware slot while
Orpheus keeps running, checks it, and restarts into it. If the new version
does not start, the player can go back to the previous one (Recovery).</p>
<p><label>Update file (optional; otherwise the release's): <input type="file" id="inst-file" accept=".ofw"></label></p>
<p><button type="button" id="inst-update">Update Orpheus</button> <button type="button" id="inst-recovery">Restart into recovery mode</button></p>
</section>
<section id="inst-adfu" hidden aria-label="Recovery mode">
<h2>1. Backup</h2>
<p><button type="button" id="inst-backup">Back up the whole flash</button> or <label>use a backup file you kept: <input type="file" id="inst-backup-file" accept=".bin"></label></p>
<p id="inst-backup-info"></p>
<p id="inst-download"></p>
<p><label><input type="checkbox" id="inst-saved"> I have saved the backup file somewhere safe.</label></p>
<h2>2. Install or restore</h2>
<p><label><input type="checkbox" id="inst-confirm0"> I agree that the next step rewrites the player's boot record.</label></p>
<p><button type="button" id="inst-first" disabled>First install of Orpheus</button> <button type="button" id="inst-restore" disabled>Restore the original firmware</button></p>
<h2>Recovery</h2>
<p>After an update that does not start, the player ends up here by itself
(after three failed starts). Start the other firmware slot, or put the
release into slot A again.</p>
<p><button type="button" id="inst-slots-read">Show the firmware slots</button> <button type="button" id="inst-switch" disabled>Start the other firmware</button> <button type="button" id="inst-reinstall">Reinstall Orpheus</button> <button type="button" id="inst-reset">Restart the player</button></p>
<p id="inst-slots"></p>
</section>
</div>
<noscript><p>This page needs JavaScript and WebAssembly.</p></noscript>
<script type="module" src="install/app.js"></script>

## Recovery mode

Updates of a running Orpheus need nothing special. Everything else happens
in the player's **recovery mode**, built into its chip and always
reachable:

* From Orpheus: the button above, or *Settings › System › Recovery mode*.
* From the original firmware there is no button: the first time, use the
  command line once (`cargo xtask dev adfu`, Linux), then continue here.
* If nothing starts: hold *Next* while plugging the cable in.

**Windows:** recovery mode has no driver. Install the WinUSB driver for the
device `10D6:10D6` once (for example with Zadig), then reload this page.
Orpheus itself needs no driver.

## The command line

Installing also works from a Linux terminal with the project's tools: the
manual has the whole procedure, including the backup and the way back.

* [Install, update and restore](manual/install.html)
* [Troubleshooting and recovery](manual/troubleshooting.html)

```sh
cargo xtask build        # writes target/orpheus/orpheus-<version>.ofw
cargo xtask dev update target/orpheus/orpheus-*.ofw
```

## Is it safe?

The YM03's USB recovery mode is in read-only memory: whatever is in the
flash chip, the player can always be reached over USB and the original
firmware put back from the backup. An update is written next to the
running firmware and only takes over once it is complete and checked; a
power cut at any moment leaves a player that starts one of the two. See
the [FAQ](manual/faq.html).
