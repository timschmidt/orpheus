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
| 10 | AU-0 | **yes, for M5** | With headphones in, run `cargo xtask dev ram audio --test audio` (about 9 s of sound) | A soft click at most when it starts; 1.5 s of a 1 kHz tone in the **left** ear only, then 1.5 s **right** only, then both ears (higher-rate part sounds identical), again both, then the tone getting quieter in 8 even steps and louder again, then silence. Report any loud pop, crackle, swapped sides or uneven steps. **Use a build from 2026-11-24 or later**: earlier builds left the amplifier gain at 0 (almost silent) | **Done 2026-11-24 (owner)**: channels, levels and steps as described. Faint clicking/ticking at regular intervals, possibly also in music; present on the Rockbox port too, not with the stock firmware → open issue AU-0b |
| 8 | INPUT-1 | **yes, for M4** | Run `cargo xtask dev ram keys --test keys --arg 60`, then press each of the ten keys once, one at a time, within the 60 s | Each key's box turns yellow while held and stays outlined green afterwards; the screen labels are PWR BCK MNU OK PRV / NXT UP DN V+ V-; report any key that lights the wrong box or none. Also say which physical key is which (e.g. "the key marked ⏮ lights PRV") | |
| 11 | PWR-3a | no | Unplug USB. On the running player hold **Power** about 4 s (Home or any screen) until the screen goes dark. Wait 10 s. Then hold Power about 1 s; afterwards plug USB in again | The screen goes dark and the player stays off (no backlight, no sound) until the next press; holding Power ~1 s starts it again (Orpheus Home); plugging USB in while off also starts it. Report if it restarts by itself, stays dark after a press, or needs the reset pinhole | |
| 12 | AU-6 | no | With headphones, play any MP3 from Files, then open Settings → Sound and change Equaliser (e.g. Rock), Bass, Crossfeed | The sound changes smoothly (no clicks or pops at the change), no dropouts | |
| 13 | AU-5 | no (gate for M5) | Unplug USB. In Files open an album folder and start its 2nd track. Listen 1 min; hold Next about 2 s (seek forward), tap Prev after 5 s (track restarts), tap Prev again (previous track). Menu → Bookmark here. Then hold Power ~4 s (power-off), start the player again, open Music | Seeking is audible within a second; after the restart Music shows **Resume ‹title›**; choosing it continues the same track within about a second of where it was switched off, and Next continues in the folder's order; Music → Bookmarks lists the bookmark with its time and title and plays from there. Report any track change that skips or repeats a track, or a resume that starts elsewhere | |
| 14 | AU-14 | no | Copy a few photos (a phone JPEG, a PNG screenshot, one portrait photo) to a folder on the card, e.g. `/Pictures/`. Stop the music. Home → Pictures → that folder → OK on a photo; press Next/Prev, OK (fill/fit), Down (rotate), Menu → Slideshow. Then play an album that has a `cover.jpg` (or embedded cover) and open Now Playing; Down for Big art | Each photo appears within a few seconds, upright (phone photos too), whole and centred with black bars; OK fills the screen, Down turns it by 90°; the slideshow moves on by itself. With music playing a photo shows "Stop playback to view pictures". Now Playing shows the album cover (small, then full screen in Big art); the second track of the album shows it at once. Report wrong colours, sideways photos, a frozen screen or a picture that never comes | |

| 14 | BT-2 | no | Put a Bluetooth headset or speaker in pairing mode next to the player, then run `cargo xtask dev cmd "bt on" --wait 20`, `cargo xtask dev cmd "bt scan" --wait 35`, `cargo xtask dev cmd "bt connect 0" --wait 30`, play a track from Files, press the headset's play/pause, then `cargo xtask dev cmd "bt off" --wait 3` | The scan lists the headset by name; connect succeeds; the music plays on the headset; its play/pause key pauses the player. Report the reply lines if not | |

| 15 | PWR-3a | no | Settings → Power → Auto-off: 5 min. Unplug USB, leave the player on Home without pressing a key and without music playing | After about 5 minutes the screen goes dark and the player stays off; holding Power ~1 s starts it again. (Low-battery shutdown, below 3.30 V for 10 s on battery, is the same path; report if the player ever switches itself off with charge left) | |
| 16 | AU-12 | no | Unplug USB. Start the player, stop any music and leave it on Home for 3 minutes (the first library build reads the tags of every song; `cargo xtask dev cmd status` meanwhile is fine). Then Music → Artists → an artist → an album → OK on its 2nd song; Back to Music → Songs, hold Next (letter jump); Music → Shuffle all; in Now Playing Menu → Go to album, Back, Menu → Go to artist. Then connect as a card reader, copy one new album, eject, wait 1 minute on Home, and look for it under Albums | Music shows "Building library…" at first, then the Artists/Albums/Songs/Genres/Recently added rows work; names are the tags (folder names where a file has none); the album's songs are in track order and playing starts at the chosen one and continues with the 3rd; Shuffle all starts at once with songs from everywhere; Go to album lands on the playing song, Go to artist on its album; the copied album appears after the card-reader session. Report the time the first build took, any wrong or garbled names, a frozen screen, or a list that never fills | |
| 17 | UI-6a | no | Unplug USB, wired headphones out. Home → Bluetooth: switch Bluetooth on (wait for "Not connected"). Put a Bluetooth headset or speaker in pairing mode, then Pair new device…: watch the list fill, OK on the headset. Play a track; change the volume on the player and on the headset; press the headset's play/pause. Then Paired devices → Menu → Disconnect; OK on it to connect again; switch Bluetooth off and on (Reconnect automatically on). Finally plug wired headphones in while it streams (carefully, low volume first) | Devices appear one by one, unnamed ones show an address (written like `E0:AD:47:…`) and get their name after the search; OK shows "Connecting…" with a spinner, then back with a toast "Connected · NAME"; music is audible on the headset at once (not silent), the player's volume keys change the headset's level and the headset's own volume moves the player's; play/pause works; after Bluetooth off/on it reconnects by itself within ~10 s. Report whether the wired headphones also play while streaming and how loud (U-BT-2), any distortion at full headset volume (DAC reference −3 dB), and a device that pairs but never connects (the failure dialog should say so) | |
| 18 | AU-5 | no | The lead uploads three album-split tracks per codec to `/.orpheus/test/gapless-flac/`, `-mp3/`, `-vorbis/`, `-wav/` (one 5-s piece of music cut into three files). With headphones, unplugged: Files → Menu → Show hidden → `.orpheus` → `test` → `gapless-mp3` → OK on the first file; listen to the end. Repeat for the other folders. Then play any album; in Now Playing → Down to Up next, Menu → Move down on the next song, Menu → Remove on another; in Files on a song Menu → Play next, on another Menu → Add to queue | Each folder sounds like **one** continuous 5-s piece: no click, gap, repeated bit or dropout where one file ends and the next begins (the title changes there). Up next shows the moved/removed/added songs at once and playback follows that order, the current song never stops. Report any audible join (which folder, after which file) | |
| 19 | PWR-5 | no | Battery drain per Power mode (D-4 as revised 2026-11-26), **with the cable unplugged** (on USB the player runs from the cable and the battery does not discharge). Pick one music folder of at least ~10 songs; Bluetooth off; wired headphones. **Run 1**: charge until Settings → Power → Battery says Charged. Settings → Power → Power mode: **Automatic**. Settings → System → About: press OK five times ("Developer on"); Settings → System → Developer → Battery log: **on** (Keep awake can stay off: playing music keeps auto-off away). Settings → Playback → Repeat: All. Unplug USB. Files → the folder → OK on the first song; set the volume to a level you note down and do not change it; short press Power to turn the screen off. After **2 hours**: wake the screen, Developer → Battery log: **off** (writes the last row). **Run 2**: plug in and charge until Charged again (if possible), then the same with Power mode: **High performance** (288 MHz all the time; same folder, same volume, screen off, 2 hours, Battery log off at the end; after a restart the About × 5 unlock is needed again). Then plug USB in and tell the lead | Developer → File shows `drain-N`; Rows written shows 1 after the first minute, then goes up by 5 every 5 minutes. Music plays without dropouts in both runs (report any, and in which run). The files are `/.orpheus/test/drain-N-auto.csv` and `drain-M-high.csv` (a third run in **Battery saver** gives `drain-K-saver.csv`) (the lead reads them back and runs `cargo xtask drain report`). Report the two start and end times, the volume level, and whether the player switched itself off | |
| 20 | FM-1 / UI-5a | no | Radio with **wired earphones in: their cable is the antenna** (unplug USB if you can, else note that it was in; never connect the jack to equipment grounded with the computer). First Settings › Radio › Region: Europe (or yours). Home → Radio (third icon); it starts by itself within ~1 s. **Hold Next** (½ s): it seeks to a station. Listen. Then: tap Next / Prev (one step each), hold Prev / Next (seek both ways); Vol+ / Vol− through the whole range; OK (mute) and OK again; on a stereo station Menu → Force mono on, then off; Menu → Auto-scan (→ Scan), wait for the "N stations" note; Up / Down step through the presets; Menu → Presets… and pick one; pull the earphones out for a few seconds and plug them back; Back to Home (the radio keeps playing, FM in the status bar); Music → play any song (the radio stops); Home → Radio again; finally hold OK (radio off) | Local stations play clearly; "ST" shows on strong stations and Force mono audibly narrows them; the volume rises evenly over the steps with no click and no sudden jump near the top; no loud pop when the radio starts, stops, or the music takes over; stations that send RDS show their name (and a scrolling text) within a few seconds; after Auto-scan the presets are the stations you can hear, strongest first; without earphones the screen says "Plug in earphones: the cable is the antenna" (and reception gets worse); music and radio never play together. Report: the region, 2–3 station frequencies and how they sounded, stereo yes/no, any whistle or hum (especially near 96.0 MHz), the loudest comfortable volume step, any pop, and whether the earphone note was right | |
| 21 | SENS-1 / UI-5c | no | Steps, unplugged. (a) Home → Steps (the count shows; Settings → Date and time must be set, else the strip says "No clock: no history"). Note the count, put the player in a trouser pocket or hold it, walk exactly **200 steps** counting them yourself, stop, look at the count. Do it **twice** (once in the hand, once in a pocket if you can). (b) Leave the player lying on a table, screen off, for **10 minutes**; note the count before and after. (c) Optional, for the detector's tests: `cargo xtask dev cmd "acc trace 120" --wait 2`, walk counting your steps for about a minute, stand still, then after the 2 minutes tell the lead your count (the trace is `/.orpheus/test/acc-N.csv`) | (a) Each walk adds 190–210 to the count (the first few steps appear together after about four steps). (b) The count does not change at all. Report both differences, where the player was carried, and whether the count went up while you only handled it (picking it up, putting it down) | |
| 23 | WEB-2 | no | The Try it page (timschmidt.github.io/orpheus/try.html, after the lead's push) in **Safari** on a Mac (and, if you can, on an iPhone or Android phone), and once more in Chrome or Firefox: wait for the Home screen, press a key, open Files › Music › Orpheus demo and play Lyre Study; change Settings › Sound › Bass; reload the page; drop an MP3 of your own onto the player and play it from Files › Your files; then Factory reset | Home within about 2 s; music starts with no crackle or gap while you browse; the bass change is audible; after the reload the setting and your file are still there and Music offers to resume; Factory reset removes them. Report the browser and version, any crackle, keys that stick, and the screen scaling (sharp pixels, no blur) | |
| 22 | UI-5d | no | Stopwatch, Timer, Alarm, unplugged, wired headphones at a low volume. (a) Home → Stopwatch: start it on OK together with a phone's stopwatch, take two laps (Next), stop both after about 10 minutes (OK) and compare. (b) Press Prev twice (the Alarm tab, then the Timer tab), choose 1:00 with Up/Down, OK, leave the app (Back) and turn the screen off (Power). (c) Back in Stopwatch, Prev once (Alarm tab): on the first alarm Menu → Time… set it two minutes ahead, OK; keep the player on (screen off is fine) and wait | (a) The two stopwatches agree within about 0.1 s after 10 minutes; laps show lap time and split. (b) After a minute the screen lights by itself showing "Time's up", the backlight flashes and a beep pattern plays in the headphones; OK stops it. (c) At the alarm minute the same with "Alarm HH:MM"; Next snoozes it (it rings again 9 minutes later). Report the stopwatch difference, a missing or late alert, and how the beeps sound (too loud, too quiet, distorted) | |
| 24 | OTP-2 / USB-5 | no | Authenticator with a **test** account (not a real one): on a computer, create a throw-away account on a site that offers TOTP two-factor sign-in (or use a TOTP test page) and save its `otpauth://` text (most sites show it under "can't scan the code?"). Put that one line into a file `/.orpheus/otp-import.txt` on the card (`cargo xtask dev put otp-import.txt /.orpheus/otp-import.txt`). Set the player's clock from the computer (`cargo xtask dev time --set`). Home → Authenticator: read the note, choose a 4–8 digit PIN, enter it again. Menu → Import from card. Compare the code with the site's or a phone authenticator's for the same account. Then, on a computer **without** the project's udev rule (or with its keyboard line removed), with a text field focused: OK on the account (big code), OK again. Finally sign in to the test site with a typed code. Back (locks), wait 60 s, reopen; enter a wrong PIN twice | The note about the PIN's limit is shown once; the import says "1 added" and "File deleted"; the code matches the phone's every 30 s and the bar shrinks to red in the last 5 s; OK types the 6 digits and Enter into the focused field (US layout; Settings › USB › Keyboard layout for others) and the site accepts it; the first USB use asks the computer to accept a keyboard "YM03 keyboard" (macOS may show its keyboard assistant: close it). A wrong PIN shows "Wrong PIN", the second one "Wait 2 s". Report a code that differs, typed characters that differ (and the layout), or no keyboard appearing | |
| 25 | AU-13 / UI-5b | no | Recorder, unplugged if you can, wired earphones in (needed for the radio; keep the volume low). (a) Home → Recorder (fifth icon): speak towards the player and watch the meter; Menu → Format: WAV, Quality: Standard; OK starts, talk for 30 s with a pause, OK (pause) for 5 s, OK again, Next once (a mark), talk 10 s more, Back → Stop. (b) Menu → Format: FLAC, record 20 s of speech the same way. (c) Home → Radio, tune a clear station, Back; Home → Recorder, Menu → Source: FM radio; OK, record 30 s, Back → Stop. (d) Menu → Recordings…: OK on each recording and listen; Menu → Delete… on one of them. (e) Copy `/Recordings` to the computer (card reader) and play the files there | (a) The meter moves with your voice (peaks around −20…−6 dB at a normal distance; red only when you shout close to it); the timer stops while paused; a "Saved · REC-YYYYMMDD-HHMMSS.wav" note at the end. (b) Same, a `.flac` file. (c) The radio keeps playing while it records; the file is called `FM-<MHz>-…` and is stereo. (d) Each recording plays back at the right speed and pitch, without clicks, gaps or repeated bits; the deleted one is gone from the list. (e) The files open in a desktop player (VLC, Audacity) with the right length. Report: how loud and clear the microphone is (noise, hum, distortion), whether the radio recording sounds like the radio, any "N dropped" on the screen, and how long the files are against how long you recorded | |
| 26 | UI-8 | no | List roll-around, after the next install. Settings: on the first row press Up, then Down; hold Down for two seconds and let go, press Down once more. Files › Music (or a big folder): Up on the first entry. Any context menu (Menu in Files): Up on the first item. Then Settings › Display › List roll-around: OK to switch it off, Back, and repeat Up on the first row of Settings | With it on: Up on the first row jumps to the last, Down on the last to the first; the held Down stops at the bottom (it does not race round), the next press goes to the top; in the big folder the last file shows after a moment. With it off: Up on the first row does nothing. Report any list that does not roll round, and whether it feels right or should be off by default | |

| 26 | USB-6 / UI-6 | no | Start a computer from an ISO on the player (only if the lead's QEMU boot test could not run, or to confirm a real PC). With the card reader (File transfer), make a folder `Images` at the top of the card and copy one **hybrid Linux ISO** into it (e.g. a current Debian netinst or Ubuntu desktop ISO; a Windows installer ISO is a second, optional try). Eject. On the player: Home → USB Drive → Images → OK on the ISO → Connect; plug the player into a PC that is switched off; switch it on and open its boot menu (F12/F11/F8/Esc, or hold Option on a Mac); choose the "Orpheus USB Drive" entry; let the installer or live system start (do **not** install anything). Then power the PC off, press Back on the player. Repeat once with Mode: Disk (OK on Mode until it says Disk) | The player shows the ISO's name, "CD-ROM · Read-only", then "Connected" and the "Read" figure growing while the PC starts; the boot menu lists the drive (UEFI and/or legacy); the installer's or live system's first screen appears. With Mode: Disk a hybrid ISO also starts on most UEFI PCs. After Back the player's music is there again. Report: the PC (make, UEFI or BIOS), the ISO, CD-ROM and/or Disk working, how long it took to reach the first screen, the read rate shown, and anything that hung | |
| 27 | UPD-1 / WEB-3 | no | **A first-time user updates from the page** (ask someone who has not used the project's tools; the lead first publishes a release with `install/release/manifest.json`, or gives them an `.ofw` file). With Orpheus running and the player plugged in, open the site's **Install** page in Chrome or Edge on their computer (Windows, macOS or Linux with the udev rule) and let them follow the page alone: Choose the player → Update Orpheus. Do not help unless they are stuck for more than a minute. Afterwards on the player: Settings › System › About shows the new version. Second part, on the player only: copy the same `.ofw` file to the card's top folder, Settings › System › Update firmware, choose it, OK, Install | The page finds the player without a driver (Chrome may first offer the page as the device's landing page), shows the progress, restarts the player and says "Done: Orpheus … is running" within about 2 minutes; the player's music, settings and card are unchanged. On the player: the file is listed with its version, OK shows its change list, Install shows checking, writing and verifying with a bar, then "Update installed" and a restart into the new version. Report every place they hesitated or misread something, the browser and operating system, and anything that needed a cable re-plug | |

| 28 | USB-W1 | no | USB comes back by itself, after the next install. With music playing and the player on the computer (any personality is fine; the lead's tools need not run): (a) pull the cable out and put it back in, five times, sometimes quickly (under a second), sometimes after 10 s; (b) put the computer to sleep for a minute and wake it; (c) plug the player into a phone charger for a minute, then back into the computer; (d) leave it plugged in, untouched, for an hour | Each time the computer sees the player again within about 5 s (the developer channel answers, or the card shows up again in File transfer); the music never stops and the player never restarts (on (d) as well: an idle computer is not a reason to restart). Report any time the computer did not see it again, any restart (the start-up screen, or the music starting from the beginning), and how long it took | |
| 35 | GAME-1…6 | Play Blocks, 4096, Solitaire, Salvo and Bricks by hand (Home › Games): moves, saves and Continue, settings | All play correctly | **Owner 2026-11-27: "I tested games, which all worked well."** |
| 36 | TTS-3 | **yes, for the screen reader on the player** | A decision, then a listen: see §36 below | | |

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

## 29. Recordings list after a delete (2026-11-25)

Record a few seconds in the Recorder, open Recorder › Recordings…, delete
the recording: the list must no longer show it (fixed in firmware; a test
may not write `/Recordings`).

## 30. Display, Power, Date & time and USB settings (VER-13 follow-up, 2026-11-26)

After the next install, unplugged where it says so. Each setting goes back
to how you had it afterwards.

* **Backlight timeout / Lock delay** (Settings › Display): set 5 s and 5 s,
  leave the player alone on Home: the screen goes dark after about 5 s; a
  key within the next 5 s only lights it again (nothing else happens);
  wait 10 s: the next key shows the lock notice, Power unlocks. With
  *Always* the screen stays on.
* **Screen saver = Clock**: at the timeout a dim clock appears on a dark
  screen instead of going dark, and moves a little at the next minute;
  any key brings back the screen; Power makes it dark as before.
* **Rotate**: on, the picture turns upside down at once (whole screen,
  clean edges, no stripes while lists scroll or pictures load) and Up/Down,
  Prev/Next swap. Report any band left upright or flicker.
* **Keys when locked**: with music playing, lock with Power. *None*: Vol±
  show the lock notice. *Volume and transport*: OK pauses/plays, Next/Prev
  change the track, the screen stays dark.
* **Power mode**: *Battery saver*: long names in lists no longer scroll;
  the player runs at 24 MHz (48 MHz with the USB cable in): screens open
  more slowly, some files may stutter or stop ("too slow"), MIDI may lose
  instruments; nothing hangs or restarts.
  *High performance*: quicker screens; the player runs at 288 MHz all the
  time (warmer, shorter battery). *Automatic*: MP3 plays smoothly, MIDI
  keeps its instruments.
* **Time zone**: set your offset; the Authenticator's codes still log you
  in (the clock shows local time, the codes use UTC).
* **Set from RDS**: on, play a station that sends the time for two
  minutes: the clock is set (check against a phone).
* **Read-only card**: on, connect to a computer, choose File transfer (a
  lock shows on that row): the computer shows the card read-only; copying
  a file onto it fails.
* **Developer channel** (only with the lead present): off, the lead's
  tools no longer find the player while File transfer still works; on
  again, they find it at once.

Report anything that does not match.

## 31. Accessibility: large text, high contrast, reduced motion, hold and repeat, key confirmation (A11Y-2, 2026-11-26)

After the next install. Each setting goes back to how you had it
afterwards (Settings › Accessibility is the last group; the themes are in
Settings › Display › Theme).

* **Text size** Large, then Extra large: Settings, Files, Music and their
  menus show four (Large) or three (Extra large) taller rows; Next/Prev
  page by what shows; long names end in "…" and scroll when focused (with
  Animations on). Home's app name and the mini player in the bottom strip,
  dialogs (e.g. Settings › System › Factory reset…, then Cancel) and Now
  Playing (title, artist, times) are larger too. Say whether the extra
  large letters (the normal font doubled, a little blocky) read well on
  the panel, and whether anything is cut that should not be.
* **Theme** White on black and Black on white: text, focus bar and
  buttons crisp and readable in daylight and in the dark.
* **Animations** off (reduced motion): no scrolling names; the recorder's
  dot stays lit while paused; an alarm's icon stays lit while ringing.
* **Hold delay** 1 s and 2 s: a short Menu press opens the menu, only a
  long one the quick panel; holding Down in a list starts repeating only
  after the delay; Power's menu needs 2 s at the 2 s setting.
* **Key repeat** Slow and Slower: holding Down in Settings moves the
  focus clearly slower (Slower: two rows a second); Normal as before.
* **Key click** "Presses and holds": a click at each press and a second
  click when a held key reaches its hold delay (try with Hold delay 2 s);
  "Presses": one click as before.

Report anything that does not match, and which text size you would use.

## 32. Brightness against the reference player (VER-13 follow-up, U-PWM-3, 2026-11-26)

After the next install, both players side by side in the same light (the
reference player on its own battery, **never** on the workstation's USB,
CLAUDE.md rule 9). Changing the reference player's brightness through its
own menu is your choice (the tools never touch it); if you would rather
not, compare only at the level it has now, with Orpheus at the same level.
Orpheus starts at level 6, the original firmware's default; a brightness
chosen in Orpheus before this install is not carried over.

* Open Settings › Display › Brightness on Orpheus and the original
  firmware's brightness setting on the reference player. Set both to the
  same level and compare by eye, for **every level 1 to 11**: the two
  screens should look equally bright at each level (Orpheus uses the
  original firmware's own PWM values: 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15
  of 16). Note any level that looks brighter or darker on one of them,
  any flicker, and whether a step changes the light at once (it should,
  while you hold the slider) or only on leaving.
* Set an alarm or a 1-minute timer on Orpheus and let it ring: the
  backlight flashes bright/dim every half second. The bright phase should
  look like level 11 (the original firmware's maximum), not brighter.
* Set Orpheus to the level you like; the reference player back to how it
  was.

Report the levels that differ (which player looks brighter), and anything
else that does not match.

## 33. MIDI files on the player (SYN-3, 2026-11-26)

After the next install and the lead's `xtask dev suite --setup midi` (it
puts three short songs of ours under `/.orpheus/test/suite/midi/`), with
headphones at a low volume:

* The lead starts the folder (`cargo xtask dev cmd 'playdir
  /.orpheus/test/suite/midi'`: Files hides `.orpheus`), or copy the three
  files (the lead can give them to you) to a folder of your music in card
  reader mode and open it in Files: it plays as a queue (Etude, Hill Song,
  Sound tour).
  Listen through all three: piano, strings, bass and drums in the etude,
  a flute melody in Hill Song, every instrument of the sound set in Sound
  tour. Expect clean sound — no clicks, dropouts, stuck notes or sudden
  silences — and the same music as the WAVs the lead can render on the
  computer (`cargo run --release -p orpheus-midi --example render --
  DIR`).
* Now Playing shows the title, `MIDI` and the duration. Pause and resume
  (Fade on pause applies), seek forward and back a few times (a short
  silence while it finds the place is expected; the notes held there
  start again), skip to the next and previous song.
* While a song plays, browse Files and Music for a minute: the music must
  not stutter.
* If you have your own `.mid` or `.kar` files on the card, try a few
  (they play with our own sound set, not the original instruments; lyrics
  are not shown yet).

Report anything that sounds wrong (which song, roughly when), and how
seeking and pausing behave.

**Result (owner 2026-11-27):** "I played midi files which all worked well as well."

## 34. The screen reader's speech (A11Y-1, 2026-11-26; on the computer, no player)

The screen reader runs in the simulator only (the player has no speech
output yet). The lead can give you eight WAV files (16 kHz mono, written by
`ORPHEUS_SPEECH_WAVS=DIR cargo test -p orpheus-sim --test screen_reader
spoken`), one per journey, with the text of each beside it
(`sim/golden/speech/*.txt`: each line is the time in ms and what was
said):

* `music_to_now_playing`: Home → Music → Artists → an artist → an album →
  a song → Now Playing.
* `settings`: Settings › Display, down the rows, a choice, back.
* `speech_rate`: Settings › Accessibility › Speech rate, raised from 1× to
  2× in 0.25× steps (each step says its value at the new speed).
* `dialog`: the factory-reset question and its buttons.
* `lock`: Power locks, a key says the lock notice, Power unlocks.
* `chords`: hold Back and Menu 2 s (reader on), Back and OK (where am I),
  Back and OK longer (time and battery), Back and Menu again (off).
* `screen_off`: the screen goes dark and the keys still work.
* `interrupt`: keys pressed quickly cut what is being said.

Listen for: is every phrase understandable at 1× (roles such as "submenu",
"switch, on", positions "3 of 12")? Are the short sounds (entering a
screen, going back, a menu, a list rolling round) helpful or annoying? Is
too much or too little said? Which speech rate would you use? Please also
say whether Back + Menu (2 s) is a good toggle and Back + OK a good "where
am I" (Vol+ with Vol− cannot work: the player sees only one of those two
keys at a time).

## 36. Speech on the player: what makes room, then a listen (TTS-3, 2026-11-27)

The screen reader and its speech voice are built for the player but do not
fit its code space next to everything already there: they need about
**41 KB** more paged code than the player can address (133 of 128 pages;
`docs/STATUS.md` 2026-11-27 "TTS-3"). Please decide what makes room:

* compile out one feature for now (approximate code each frees: Opus
  ≈ 61 KB, the MIDI player and synthesiser ≈ 65 KB, the games ≈ 55 KB,
  Pictures ≈ 45 KB), or
* let the lead plan a pager change so rarely used code can also run from
  the read-only data window (about 147 KB of room; a larger, riskier
  package), or
* leave speech out of the player until then.

Once a build with speech fits (`cargo xtask build --features speech`), with
headphones: hold **Back + Menu** 2 s on Home (a rising tone, "Screen reader
on", "Home. Music, button, 1 of 12"); move through Home, Settings ›
Accessibility and a list in Music; hold **Back + OK** (where am I) and
longer (time and battery); press keys quickly while it speaks (each cuts
it); try Speech rate 2× and 0.5×, Voice, Speech volume −12 dB, and Music
while speaking (Lowered, Paused) with a song playing; Back + Menu again
(off). Report: understandable at 1×? the tones helpful? speech too loud or
quiet against the music, the lowered music level, the pause's resume
delay; any crackle, gap or delay before the first word.
