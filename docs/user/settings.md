# Settings

Settings are grouped by topic. Changes apply at once and are kept when the
player is switched off; there is no Save button. Up and Down move, OK opens
or changes, Back goes up a level.

![Settings](../shots/app_settings_dark_latin.png)

## Groups

| Group | What is in it | State |
|---|---|---|
| **Sound** | Equaliser, balance, bass, crossfeed, channel mode (stereo, mono, swapped), ReplayGain, volume limit, key click, fade on pause | Works |
| **Playback** | Shuffle, repeat, gapless, resume at start-up, seek step, bookmark long files, tag code page | Works; speed is hidden until it is built |
| **Display** | Brightness, backlight, auto-lock, theme, accent colour, screen saver (a dim clock), animations, rotate 180°, clock in status bar, list roll-around, Home screen apps | Works; wallpaper is **in progress** |
| **Power** | [Power mode](power.md), sleep timer, auto-off, keys when locked (none, volume, volume and playback), battery | Auto-off and low-battery switch-off are **being checked** unplugged |
| **Date and time** | Time, date, 24-hour clock, date format, time zone, clock correction (seconds a day), set from radio (RDS) | Works; the clock has stopped after some restarts (known issue: set it again) |
| **Bluetooth** | On/off, device name, reconnect, output | See [Bluetooth](bluetooth.md). Output does not change the route yet: sound follows the Bluetooth connection |
| **Radio** | Region, force mono, seek sensitivity | See [Radio](radio.md) |
| **Recording** | Source, format, quality, gain, voice activation, auto split, monitor, low cut | See [Recorder](recorder.md); monitor is **in progress** |
| **USB** | When connected (ask, charge only, file transfer, keyboard, last disk image), read-only card reader, developer channel, keyboard layout, digits from keypad, Enter after code | Works |
| **Language** | English; Russian and Japanese | English on the player; the others run in the simulator and are **in progress** on the player |
| **Storage** | Card info, rebuild library, clear caches, check card, format card | Card info and rebuild library work; the other three do nothing on the player yet (**in progress**) |
| **System** | About, update firmware, diagnostics, factory reset, recovery mode | Works; diagnostics is **in progress** |
| **Accessibility** | Text size, hold delay, key repeat, [screen reader and speech](screen-reader.md) | See [Accessibility](accessibility.md) |

![Display](../shots/app_display_dark_latin.png)

**Brightness** has the original firmware's levels, 1 to 11 (6 at first),
and changes at once.

**Theme**: Dark, Light, Amber, and two high-contrast themes, White on
black and Black on white.

**List roll-around** (on at first): in every list, Down on the last row
goes to the first and Up on the first row to the last; Prev and Next jump
round. Holding Up or Down stops at the end.

**Home screen** chooses which apps Home shows. A player set up before
Books existed keeps its old choice, so Books stays hidden until you
switch it on here.

**Key click** sounds a click for each key press, or for presses and holds
(a second click when a hold has registered). **Fade on pause** fades the
music out in 60 ms instead of cutting it.

## Equaliser

Sound › Equaliser lists the presets: Flat, Rock, Pop, Jazz, Classical,
Electronic, Hip-hop, Bass boost, Vocal and two custom curves. A custom
curve has five bands (60 Hz to 14 kHz, ±12 dB): Prev and Next choose a
band, Up and Down change it. The pre-amp follows so the sound does not
clip.

![Equaliser](../shots/media_eq_dark_latin.png)

## System

* **About**: version, build, firmware slot, serial number, hardware.
* **Update firmware…**: installs an update file from the card
  ([install](install.md#update)).
* **Factory reset**: every setting back to its default. Music, the library
  and bookmarks on the card are not touched.
* **Recovery mode**: restarts into the chip's USB recovery mode, for the
  install tools ([troubleshooting](troubleshooting.md)).

Pressing OK five times on About shows a **Developer** group until the next
restart (battery logging for tests). It is not needed for normal use.
