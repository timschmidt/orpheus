# Checks that need a person

Workers cannot see the screen, hear the output, press keys or re-plug cables.
Each entry says what to do, what to expect, and which work package is waiting.
Workers add entries (with the exact file or screen to use); the user fills in
the result and date. "Blocking" entries hold up the package named.

## Open now (useful before any code exists)

| # | For | What to do | Expect | Result |
|---|---|---|---|---|
| 1 | UI-1 | Describe or photograph the player's keys: where each is and what is printed on it | Ten keys: power, back/return, menu/mode, play, previous, next, up, down, volume up, volume down | |
| 2 | FS-2 | Keep a backup copy of the music card before file-system write tests start (milestone M3) | — | |
| 3 | BT-3 (optional) | With the **stock** firmware in Bluetooth mode (hold OK while switching on to start stock), scan from a phone: is the player visible, under what name? | Helps decide whether the module can accept incoming connections | |
| 4 | BT-2 (optional) | With the stock firmware and a Bluetooth headset connected: does the wired headphone output still play? Does the Bluetooth level follow the player's volume? | Locates the analogue tap | |
| 5 | PWR-4 (optional) | If a USB power meter is at hand: charging current under the stock firmware with a part-empty battery | About 200 mA expected from the datasheet | |
| 6 | D-5 (optional) | Decide whether the case may be opened for a board inspection | See `docs/PLAN.md` §5 | |

## Requested by work packages

| # | For | Blocking? | What to do | Expect | Result (user) |
|---|---|---|---|---|---|
| 7 | LCD-1 | no | With the player on USB, run `cargo xtask dev ram lcd --test lcd` and look at the screen after it finishes (the picture stays until the player is switched off) | White frame; colour bars left→right white, yellow, cyan, green, magenta, red, blue, black; a grey ramp dark→light; below, a gradient red→ to the right, green downwards, blue on the left; no tearing, sharp edges, upright (text-free) | |
| 9 | PWR-2 | no | If a USB power meter is at hand: run `cargo xtask dev ram charge --test charge --arg 120` with a part-empty battery and read the meter during the 2 minutes (the screen says CHARGING 200MA) | Charge current about 200 mA (never clearly above); the screen shows battery mV rising slowly | |
| 10 | AU-0 | **yes, for M5** | With headphones in, run `cargo xtask dev ram audio --test audio` (about 9 s of sound) | A soft click at most when it starts; 1.5 s of a 1 kHz tone in the **left** ear only, then 1.5 s **right** only, then both ears (higher-rate part sounds identical), again both, then the tone getting quieter in 8 even steps and louder again, then silence. Report any loud pop, crackle, swapped sides or uneven steps | |
| 8 | INPUT-1 | **yes, for M4** | Run `cargo xtask dev ram keys --test keys --arg 60`, then press each of the ten keys once, one at a time, within the 60 s | Each key's box turns yellow while held and stays outlined green afterwards; the screen labels are PWR BCK MNU OK PRV / NXT UP DN V+ V-; report any key that lights the wrong box or none. Also say which physical key is which (e.g. "the key marked ⏮ lights PRV") | |
| 11 | PWR-3a | no | Unplug USB. On the running player hold **Power** about 4 s (Home or any screen) until the screen goes dark. Wait 10 s. Then hold Power about 1 s; afterwards plug USB in again | The screen goes dark and the player stays off (no backlight, no sound) until the next press; holding Power ~1 s starts it again (Orpheus Home); plugging USB in while off also starts it. Report if it restarts by itself, stays dark after a press, or needs the reset pinhole | |
| 12 | AU-6 | no | With headphones, play any MP3 from Files, then open Settings → Sound and change Equaliser (e.g. Rock), Bass, Crossfeed | The sound changes smoothly (no clicks or pops at the change), no dropouts | |
| 13 | AU-5 | no (gate for M5) | Unplug USB. In Files open an album folder and start its 2nd track. Listen 1 min; hold Next about 2 s (seek forward), tap Prev after 5 s (track restarts), tap Prev again (previous track). Menu → Bookmark here. Then hold Power ~4 s (power-off), start the player again, open Music | Seeking is audible within a second; after the restart Music shows **Resume ‹title›**; choosing it continues the same track within about a second of where it was switched off, and Next continues in the folder's order; Music → Bookmarks lists the bookmark with its time and title and plays from there. Report any track change that skips or repeats a track, or a resume that starts elsewhere | |
| 14 | AU-14 | no | Copy a few photos (a phone JPEG, a PNG screenshot, one portrait photo) to a folder on the card, e.g. `/Pictures/`. Stop the music. Home → Pictures → that folder → OK on a photo; press Next/Prev, OK (fill/fit), Down (rotate), Menu → Slideshow. Then play an album that has a `cover.jpg` (or embedded cover) and open Now Playing; Down for Big art | Each photo appears within a few seconds, upright (phone photos too), whole and centred with black bars; OK fills the screen, Down turns it by 90°; the slideshow moves on by itself. With music playing a photo shows "Stop playback to view pictures". Now Playing shows the album cover (small, then full screen in Big art); the second track of the album shows it at once. Report wrong colours, sideways photos, a frozen screen or a picture that never comes | |

| 14 | BT-2 | no | Put a Bluetooth headset or speaker in pairing mode next to the player, then run `cargo xtask dev cmd "bt on" --wait 20`, `cargo xtask dev cmd "bt scan" --wait 35`, `cargo xtask dev cmd "bt connect 0" --wait 30`, play a track from Files, press the headset's play/pause, then `cargo xtask dev cmd "bt off" --wait 3` | The scan lists the headset by name; connect succeeds; the music plays on the headset; its play/pause key pauses the player. Report the reply lines if not | |

| 15 | PWR-3a | no | Settings → Power → Auto-off: 5 min. Unplug USB, leave the player on Home without pressing a key and without music playing | After about 5 minutes the screen goes dark and the player stays off; holding Power ~1 s starts it again. (Low-battery shutdown, below 3.30 V for 10 s on battery, is the same path; report if the player ever switches itself off with charge left) | |
| 16 | AU-12 | no | Unplug USB. Start the player, stop any music and leave it on Home for 3 minutes (the first library build reads the tags of every song; `cargo xtask dev cmd status` meanwhile is fine). Then Music → Artists → an artist → an album → OK on its 2nd song; Back to Music → Songs, hold Next (letter jump); Music → Shuffle all; in Now Playing Menu → Go to album, Back, Menu → Go to artist. Then connect as a card reader, copy one new album, eject, wait 1 minute on Home, and look for it under Albums | Music shows "Building library…" at first, then the Artists/Albums/Songs/Genres/Recently added rows work; names are the tags (folder names where a file has none); the album's songs are in track order and playing starts at the chosen one and continues with the 3rd; Shuffle all starts at once with songs from everywhere; Go to album lands on the playing song, Go to artist on its album; the copied album appears after the card-reader session. Report the time the first build took, any wrong or garbled names, a frozen screen, or a list that never fills | |
| 17 | UI-6a | no | Unplug USB, wired headphones out. Home → Bluetooth: switch Bluetooth on (wait for "Not connected"). Put a Bluetooth headset or speaker in pairing mode, then Pair new device…: watch the list fill, OK on the headset. Play a track; change the volume on the player and on the headset; press the headset's play/pause. Then Paired devices → Menu → Disconnect; OK on it to connect again; switch Bluetooth off and on (Reconnect automatically on). Finally plug wired headphones in while it streams (carefully, low volume first) | Devices appear one by one, unnamed ones show an address (written like `E0:AD:47:…`) and get their name after the search; OK shows "Connecting…" with a spinner, then back with a toast "Connected · NAME"; music is audible on the headset at once (not silent), the player's volume keys change the headset's level and the headset's own volume moves the player's; play/pause works; after Bluetooth off/on it reconnects by itself within ~10 s. Report whether the wired headphones also play while streaming and how loud (U-BT-2), any distortion at full headset volume (DAC reference −3 dB), and a device that pairs but never connects (the failure dialog should say so) | |

## Template for workers

| # | For | Blocking? | What to do | Expect | Result (user) |
|---|---|---|---|---|---|
| n | WP id | yes/no | Exact steps, file names under `/.orpheus/test/` on the card, screen to open | What a pass looks or sounds like | |

## Planned checks by milestone (details are added by the package that needs them)

* **M1**: display looks right (LCD-1); every key registers once (INPUT-1,
  blocking for M4); 1 kHz tone left then right, no loud pop, even volume steps
  (AU-0, blocking for M5); charge to full terminates (PWR-2).
* **M4**: power off and on by key; wake by plugging in (PWR-3a).
* **M5**: listening test — MP3 and FLAC albums, a gapless album, seek, resume
  after power-off (gate for M5).
* **M6**: one listening pass per new codec; equaliser presets sound plausible.
* **M7**: radio reception and stereo; recordings from each source; 200 counted
  steps twice; alarm wakes the player; photos look right.
* **M8**: pair a headset and a speaker; music, remote keys, volume, reconnect.
* **M9**: a typed one-time code logs in; a PC boots
  from an ISO (only if the QEMU pass-through test could not run).
* **M10**: two weeks of daily use on a release candidate; optional current
  measurements for the battery-life table.
