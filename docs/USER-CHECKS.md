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
| 8 | INPUT-1 | **yes, for M4** | Run `cargo xtask dev ram keys --test keys --arg 60`, then press each of the ten keys once, one at a time, within the 60 s | Each key's box turns yellow while held and stays outlined green afterwards; the screen labels are PWR BCK MNU OK PRV / NXT UP DN V+ V-; report any key that lights the wrong box or none. Also say which physical key is which (e.g. "the key marked ⏮ lights PRV") | |

## Template for workers

| # | For | Blocking? | What to do | Expect | Result (user) |
|---|---|---|---|---|---|
| n | WP id | yes/no | Exact steps, file names under `/yoton-test/` on the card, screen to open | What a pass looks or sounds like | |

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
