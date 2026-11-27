# 05 — User interface and apps

This is the product specification for everything the user sees and touches. It is
written so that the UI toolkit, each app and the simulator tests can be built
from it without further design decisions. Where a feature depends on hardware
that is not yet proven (Bluetooth roles, sensor features), the dependency is
named and the screen degrades as described.

Read [02-architecture.md](02-architecture.md) §"Tasks and priorities" first: the
UI runs in thread mode at the lowest priority and may be preempted at any point
by audio work. Nothing in the UI may assume it runs on time.

## 1. Hardware the UI has to work with

| Item | Value |
|---|---|
| Screen | 128 × 128 px, RGB565, about 1.44", ~120 dpi, backlit LCD (ST7735S class) |
| Frame buffer | 32 KiB, all of SRAM1 (`0x158000`); the host tools screenshot it directly |
| Keys | Up, Down, Prev, Next, OK (Play), Menu, Back, Vol+, Vol−, Power |
| Key electrics | Two resistor ladders plus a PMU level for Power: **at most one key per ladder is seen at a time**. Ladder 1: Back, Next, Vol+, Vol−. Ladder 2: Menu, Prev, Down, OK, Up. Power is independent |
| Feedback | Backlight PWM (16 levels), audio key click through the DAC (no vibration, no LED confirmed) |

Chords are therefore limited to *Power + any key* and *one key of ladder 1 + one
key of ladder 2* (for example Back + Menu, Vol+ + OK). No chord may be the only
way to reach a function.

The stock firmware's key names (CONFIG.TXT `CFG_KEY_ARRAY`) are: power, return,
next, vol+, vol−, mode, prev, down, play, up. The printed legends on the case
have not been recorded; WP-UI-1 asks the user for a photo or description and
adjusts on-screen hints (not behaviour) to match.

## 2. Principles

1. **One grammar everywhere.** Each key means the same thing on every screen
   (§3). A screen may leave a key unused; it may not redefine it.
2. **Never lose the music.** Playback, radio and recording continue across every
   screen change. Leaving an app never stops its audio; only an explicit stop,
   another audio source, or power-off does.
3. **Two presses to anything common.** From Home: one move plus OK reaches any
   app; holding Menu reaches the quick panel from anywhere; holding Back
   returns Home from anywhere.
4. **Show state, not chrome.** No decorative frames. Every pixel row is either
   content, the status bar, or the hint/mini-player strip.
5. **Instant response.** A key press changes pixels within 50 ms even when the
   work behind it (directory read, track start) takes longer: move the highlight
   first, show a spinner row, fill in when data arrives. Lists never block on
   storage.
6. **Recoverable.** Destructive actions (delete, format, factory reset, replace
   firmware) ask once with the safe choice focused. Everything else applies
   immediately and is undone by doing the opposite.
7. **Readable at arm's length while moving.** This is a clip-on sport player:
   minimum text height 11 px, primary values (track title, frequency, step
   count, timer, one-time code) in the large font, 4.5:1 contrast minimum.
8. **Cheap to draw.** Redraw only dirty rectangles. No full-screen animation at
   more than 30 fps, none at all in Battery-saver mode, and never while the
   audio pipeline reports less than 50 % buffer.

## 3. Global key grammar

| Key | Tap | Hold (≥ 500 ms) |
|---|---|---|
| **Up / Down** | Move focus; change the focused value in editors. In every scrollable list, Down on the last row goes to the first and Up on the first to the last when **List roll-around** is on (Settings › Display; owner's requirement 2026-11-24) | Auto-repeat, accelerating after 1 s (lists: 8 → 30 rows/s); auto-repeat stops at the list end instead of rolling round, so a held key never races round a list |
| **Prev / Next** | Lists: page up / page down. Players: previous / next item. Editors: previous / next field. Tabs: previous / next tab | Lists: jump by first letter (overlay shows the letter). Players: seek / tune, accelerating |
| **OK** | Activate the focused item. Players: play / pause | Players: stop (radio: mute; recorder: stop and save after confirmation) |
| **Back** | Up one level; closes menus, dialogs, overlays | Home |
| **Menu** | Context menu for the current screen (always the same key, always a list) | Quick panel (§6.4) |
| **Vol+ / Vol−** | Volume, with overlay; works on every screen, with the screen off and when keys are locked (configurable) | Ramp |
| **Power** | Screen off + key lock; tap again to wake | 1.5 s: power menu. 4 s: power off without the menu |

Rules that follow from the grammar:

* Back never activates anything and never loses unsaved input without asking
  (text entry and the recorder are the only places with unsaved input).
* A context menu never contains navigation ("go to …") that Back or Home already
  provides, except "Now Playing".
* While the screen is off, the first key press only wakes it, except Vol±
  (and transport keys if the user enables "Transport keys when locked").
* Key repeat and hold thresholds live in one place (`ui::input`), not per app.
* Every press may produce a key click (off by default) mixed into the audio
  output; never when recording from the microphone.

**As built (VER-13, 2026-11-26): key click.** Settings › Sound › Key click
(0x0108): every key that goes down and is not dropped by the key gate
(`orpheus_ui::shell::System::key_click`) clicks — a 3 ms burst of a 2.5 kHz
triangle under a linear decay, ≈ −15 dBFS (`orpheus_dsp::click`) — unless
the microphone is live (the recorder listening for its meters or
recording from it: `orpheus_apps::services::key_click`). While music plays
the click is mixed into the output after the DSP chain; with nothing
playing (or paused) the card task sounds it alone, the DAC started for it
(as the alert tone), and drops it when card work is waiting rather than
delay that work. Not over the radio (its sound bypasses the DAC). The
player's `status` counts the clicks sounded (`clicks=`).

Power-menu content: Power off · Sleep timer · Lock keys only · Restart ·
(dual-boot layout) Start original firmware · Recovery mode (ADFU; developer
option).

## 4. Screen layout and visual language

```
┌────────────────────────────┐ y=0
│ status bar            14px │  clock · state icons · battery
├────────────────────────────┤ y=14
│                            │
│ content                    │  96 px: 5 list rows of 19 px (+1 px rule)
│                            │  or free layout
├────────────────────────────┤ y=110
│ strip                 18px │  mini-player, hints, or more content
└────────────────────────────┘ y=128
```

* **Status bar** (14 px): left, clock `HH:MM` (or the screen title when the
  clock is disabled); right to left: battery (5-segment glyph, bolt when
  charging, red below 10 %), Bluetooth (off: hidden; on: outline; connected:
  filled; streaming: filled + waves), audio source state (▶ ❙❙ ● FM), lock,
  sleep-timer moon, alarm bell, USB/SD activity dot. Icons are 10 × 10 px
  1-bit masks tinted by the theme.
* **Strip** (18 px): when something is playing and the current screen is not its
  player, the strip is a mini-player (state icon, scrolling title, thin progress
  line along its top edge). Otherwise screens may use it for a hint line
  ("OK start · Next lap") or give the space to content. Full-screen content
  (pictures, big-art Now Playing) hides both bars; a key press shows them for 3 s.
* **Rows**: 19 px pitch. Left: optional 12 × 12 type icon. Text: UI font,
  vertically centred. Right: value text, chevron `›` for sub-levels, or a
  toggle/radio glyph. The focused row is a filled rounded rectangle in the
  accent colour with the text in the on-accent colour; long focused text
  scrolls (marquee starts after 800 ms, 30 px/s, pauses 1 s at each end).
  A 2 px scroll indicator on the right edge appears when the list overflows.
* **Fonts** (see 02-architecture §"Fonts"):
  * UI: proportional bitmap, 12 px cap-to-descender box, Latin + Latin
    Extended-A + Greek + Cyrillic + punctuation and symbols (built in).
  * Fallback: 16 px bitmap for every other Basic Multilingual Plane code point
    (CJK, Hangul, kana, Thai, Hebrew, Arabic isolated forms), read on demand
    from the font pack in NOR. Rows are tall enough for it; no row changes
    height. No complex shaping and no bidirectional layout in v1: right-to-left
    runs are shown reversed as a whole so at least plain Hebrew/Arabic words read correctly.
  * Large numerals: 28 px digits, `:`, `.`, `-`, space (clock, stopwatch,
    frequency, steps, one-time codes); 16 px digits for secondary values.
* **Colour**: themes are 8-entry palettes: background, surface (bars, cards),
  text, text-dim, accent, on-accent, warning, good. Ship: Dark (default),
  Light, Black-and-amber (high contrast), plus accent variants Blue, Green,
  Pink, Red, Grey, matching the stock firmware's colour choices. All drawing goes
  through palette roles, never literal colours. *As built (A11Y-2):* two more themes,
  White on black and Black on white, at 7:1 or better for every text
  pairing (§7.14).
* **Motion**: list scrolling is by whole rows (no pixel smooth-scroll). Screen
  changes: 120 ms horizontal slide of the content area only (8 steps), skipped
  when disabled, in Battery-saver mode, or when audio buffers are low. Marquee,
  progress and the spinner are the only continuous animations.
* **Rotation**: 0° and 180° (clip worn upside down), set in Display settings;
  implemented with the panel's MADCTL and a swapped key map for Up/Down and
  Prev/Next. *As built (VER-13):* in software, without a controller flag
  (the 180° MADCTL value and its RAM offsets are not hardware facts yet):
  each flushed band of the frame buffer is turned in place, sent to the
  mirrored panel rows and turned back (`orpheus_ui::gfx::turn_band`), so the
  frame buffer stays upright for drawing and screenshots; the key mapper
  swaps Up/Down and Prev/Next. Held off under a host test's look.

## 5. Toolkit (crate `orpheus-ui`)

`orpheus-ui` is `no_std`, has no allocator, and knows nothing about hardware. It
draws through `embedded_graphics::draw_target::DrawTarget<Color = Rgb565>` and
receives `Event`s. The same crate runs on the device and in the desktop
simulator.

### 5.1 Core types

```rust
pub enum Key { Up, Down, Prev, Next, Ok, Back, Menu, VolUp, VolDown, Power }
pub enum KeyPhase { Down, Repeat, Hold, Up { held: bool } }
pub enum Event {
    Key(Key, KeyPhase),
    Tick(Instant),                 // only while a screen asked for ticks
    Service(ServiceEvent),         // playback, battery, storage, bt, usb, sensor …
    Data(DataReady),               // a row/page/image a screen asked for has arrived
}
pub enum Nav { Stay, Push(ScreenId, Arg), Replace(ScreenId, Arg), Pop, PopTo(ScreenId), Home }

pub trait Screen {
    fn enter(&mut self, cx: &mut Cx, arg: Arg);
    fn event(&mut self, cx: &mut Cx, ev: &Event) -> Nav;
    fn draw(&mut self, cx: &Cx, g: &mut Gfx, dirty: Dirty);   // content area only
    fn menu(&self, cx: &Cx, out: &mut MenuBuilder) {}         // context menu items
    fn wants(&self) -> Wants { Wants::default() }             // ticks, full screen, strip use
}
```

* Screens are variants of one `enum ScreenState` (static dispatch, no `dyn`, no
  heap). The navigation stack is a fixed array of 8 entries; each entry stores
  the screen id plus a ≤ 64-byte resume record (focused index, scroll offset,
  path handle) so that popped-to screens are rebuilt, not kept alive. Only the
  top screen holds full state. This keeps UI RAM under 2 KiB regardless of depth.
* `Cx` gives access to services through traits (`Playback`, `Library`,
  `Storage`, `Settings`, `Power`, `Radio`, `Recorder`, `Bluetooth`, `Usb`,
  `Clock`, `Sensors`, `Vault`). The device implements them by sending commands
  over channels to the owning tasks; the simulator implements them with host
  code. Screens never await: a request returns immediately and the answer
  arrives later as `Event::Service`/`Event::Data`.
* `Gfx` wraps the frame buffer with a clip rectangle, palette roles, text
  drawing with fallback-font lookup, icon blits (1-bit and 2-bit alpha masks,
  RGB565 images), and dirty-rectangle tracking. The UI task flushes the union
  of dirty rectangles after each event batch (hardware: DMA blit, see 03).

### 5.2 Widgets

| Widget | Notes |
|---|---|
| `ListView` | Virtual: asks a `ListSource` for rows `[first, first+n)`; keeps a cache of two pages. Rows not yet loaded draw as dim placeholders. Supports sections, per-row icon/value/accessory, letter jump, **roll-around** (Down past the last row → first, Up before the first → last; Prev/Next pages roll round the same way) following the **List roll-around** setting, owned by `ListView` so every list — menus, settings, Files, Music/library lists, Up next, presets, recordings, Authenticator, Bluetooth devices, pickers — behaves the same; an auto-repeated key stops at the end, a fresh press rolls round |
| `Menu` | A `ListView` in a bottom sheet over a dimmed screen; items: action, toggle, radio group, sub-menu, slider launcher |
| `Dialog` | Title, up to 3 lines of text, 1–3 buttons; destructive dialogs focus the safe button |
| `Slider` | Horizontal value with min/max/step, live apply (brightness, EQ band, gain) |
| `Picker` | Wheels for time/date/number; Prev/Next change field, Up/Down change value |
| `TextEntry` | On-screen keyboard: 4 rows × 10 keys (letters, digits, symbols layers; Base32 layer for secrets), Menu switches layer, hold OK = done; shows the text line above |
| `Progress`, `Spinner`, `Meter` | Bar with optional buffered range; indeterminate spinner; stereo level meter with peak hold |
| `Toast` | One line over the strip for 2 s ("Added to Favourites"); never steals focus |
| `VolumeOverlay` | Large number + bar, 1.2 s; draws over anything, including full-screen content |
| `Tabs` | Up to 4 labelled tabs under the status bar; Prev/Next switch |
| `BigValue` | Large-numeral field with unit and caption (clock, steps, frequency) |

**As built (UI-8, 2026-11-24): list roll-around.** The setting is
Settings › Display › List roll-around (`0x030C`, toggle, **on by default**,
between Clock in status bar and Home screen). `orpheus_ui::shell::System::
list_roll_around` reads it (apps: `services::list_roll_around`); the shell
reads it once per key into `Cx::roll`, and every list takes it as
`ListCx` from `cx.list()` into `ListView::handle` — the menu sheet too.
Rules, all in `ListView`: a fresh Up/Down (`KeyPhase::Down`) rolls round
past an end (section headers are skipped, the header above the first row
comes back into view); `KeyPhase::Repeat` never rolls round, so a held key
stops on the end row (the accelerated 5/20-row steps too); Next on the last
focusable row goes to the first, Prev on the first to the last row (other
pages clamp to the end row first: 0 → 5 → 10 → 15 → 19 → 0 in 20 rows, 5
visible); without roll-around every end clamps as before. Windowed lists (Files,
Music › Folders, Pictures, Recordings, the library) roll onto the last row
as a placeholder and load its window like any other move. Screens that draw
their own rows use the same rules through `ListView::step_plain` (the
Authenticator list and its big-code view, the stopwatch laps). Not lists,
and unchanged: the Home grid and the timer's preset chips (always wrap), the
radio's Up/Down preset stepping (always wraps), the Steps week chart (seven
bars, clamps), dialog buttons, value editors and pickers (Choice lists *are*
lists and roll). **Exception:** Now Playing's Up next and Details views
keep their behaviour in both settings — a fresh Up/Down past either end
changes the view (the views cycle round), so rolling round inside them
would trap the key in the view; a held key stops at the end. Two toolkit
changes came with it: the stopwatch/timer/alarm screen swallows the rest of
a Prev/Next press (its release no longer pages the alarm list), and the
screen under an open menu or dialog is drawn as of the moment it opened
(a marquee under the scrim no longer moves when the scrim is redrawn).

### 5.3 Text and strings

All user-visible strings go through a string table (`tr!(Id)`), English built
in. Additional languages are resource files loaded into a small arena
(`.orpheus/lang/xx.ylang` on the card, or in the NOR resource area); the table
format is defined in WP-UI-7 and is not needed before the polish milestone, but
**no string literal may be drawn directly** from the first screen on.

Tag and file-name text is UTF-8 internally. Legacy ID3 text is decoded with the
configured code page (default Windows-1252; CJK code pages are optional
resources).

## 6. System screens

### 6.1 Boot, charging, shutdown

* Cold boot shows the logo for at most 300 ms after the display is ready, then
  resumes exactly where the user left off if "Resume" is on (same screen stack
  root, playback paused at the saved position; starts playing only if it was
  playing when the device was switched off by the sleep timer or battery).
* Plugged in while off: charging screen (large battery glyph, percent, "USB
  connected"), backlight off after 10 s; Power starts the player.
* Low battery: toast at 15 % and 10 %; at 5 % a dialog; at the cut-off voltage
  the firmware saves state, stops audio with a fade and powers off.
**As built (VER-13, 2026-11-26): Resume at start-up** (Settings ›
Playback, 0x0204, on by default). Once the card is mounted and the state
loaded, and the UI has published its settings (at most 2 s), the saved
place opens **paused** (`firmware::audio::resume_at_start` →
`Server::resume_at_start`, the player's `resume(…, play = false)`): Home's
strip shows the paused track, Music offers Now Playing, OK plays on. It
always opens paused (the record's "was playing" is not used: a user's
power-off and the battery's cannot be told apart yet). The screen stack
is not restored. Off: nothing is loaded; Music offers "Resume ‹title›".
While the track is open (paused) the idle-only background work (the
library build, directory indexes) waits, as during any pause.

* Shutdown: save resume state and settings, flush the card, fade audio, "Goodbye"
  for 300 ms, PMU off.

### 6.2 Home

4 × 3 grid of 32 × 32 cells (eleven apps; the last cell is empty and is
skipped by navigation), 24 × 24 two-bit icons; the focused cell is an
accent rounded square with the icon in on-accent. The strip shows the focused
app's name, or the mini-player (then the app name shows as a toast-style label
for 1 s after each move).

| | | | |
|---|---|---|---|
| Music | Files | Radio | Bluetooth |
| Recorder | Pictures | Steps | Stopwatch |
| Authenticator | USB Drive | Settings | |

Up/Down/Prev/Next move in the grid (wrapping), OK opens. Order and visibility
are editable in Settings → Display → Home screen (stored as the apps *hidden*,
key 0x030E since 2026-11-27, so an app added by a later firmware shows until
the owner hides it; the old key 0x030B, the apps shown, is retired). Apps whose hardware is absent
or unsupported in the running build (see 06-connectivity §"Bluetooth") are
hidden, not greyed.

### 6.3 Lock and screen-off

Power tap, or the backlight timeout followed by the lock timeout, turns the
panel off (LCD sleep, backlight PWM off, UI redraws suspended). Playback
continues at the lowest sufficient clock. While locked, a key press shows the
lock screen for 2 s: clock in large numerals, date, track title, battery,
"Press Power to unlock". Optional screen saver instead of off: dim clock that
moves every minute.

*As built (VER-13):* `orpheus_ui::input::ScreenTimer` turns the screen off
Backlight time-out after the last key (the next key only wakes it), and locks
the keys Lock delay after that (Immediately, 5, 30, 60 s, Never; Power still
locks at once). With Screen saver = Clock the panel stays lit at the
time-out and the shell draws the time in dim 28 px numerals on a dark
background, moved every minute (`saver_spot`); Power always turns the panel
dark. Keys when locked: None, Volume, or Volume and transport (OK
play/pause, Next/Prev track) — transport keys go to playback
(`Gate::Transport`), never to the screen under the lock. Animations (and
Battery saver) keep marquees at rest. Under a host test's look (the suite,
the fuzzers, the soak) the timers are held unless the test session sets one
off its default (`services::TimerHold`); setting it back wakes the screen
and frees the keys.

### 6.4 Quick panel (hold Menu)

A sheet with: Now Playing (if any) · Lock · Bluetooth on/off and current device
· Sleep timer (Off, 15, 30, 45, 60, 90 min,
end of track) · Brightness slider · Shuffle · Repeat · EQ preset. Values change
with Prev/Next directly in the row, without leaving the panel. (An Output
row, Headphones / Bluetooth, was specified and is hidden since 2026-11-27:
the YM03 has no route to choose. The Bluetooth module takes its audio
after the headphone amplifier, U-BT-2, so the jack and a connected sink
always carry the same sound; the setting 0x0604 is kept in the store,
`settings::hidden`.)

### 6.5 USB connected dialog

Shown when a host (not a charger) is detected, unless a default is set:

Charge only · File transfer (card reader) · Present disk image… · Keyboard
(types one-time codes) · USB audio (if built) · [Always do this].

In File transfer mode the screen shows a large USB glyph, the transfer
direction and rate, and "Eject on the computer before unplugging". Playback
stops because the card belongs to the host. Back asks to disconnect (soft
detach) and returns to the previous screen once the host has released the card.

### 6.6 Errors

One style: icon, one-sentence cause, one-sentence action. No card: Music/Files/
Pictures/Recorder show "No card — insert a microSD card" with the radio,
stopwatch, steps, authenticator and settings still usable. Unreadable card:
offer "Check card" (read-only scan) and "Format" (Settings → Storage only).
Unsupported or damaged file: toast naming the file, then skip to the next one;
three failures in a row stop playback with a dialog.

## 7. Apps

Each subsection lists screens, key behaviour beyond the global grammar, the
context menu, persistent settings, and acceptance checks that the simulator
tests (and where marked **[HIL]**, on-device tests) must cover.

### 7.1 Music

**Root list**: Now Playing (only when a queue exists) · Resume "‹title›" ·
Shuffle all · Artists · Albums · Songs · Genres · Playlists · Folders ·
Recently added · Bookmarks.

* Artists → albums of the artist (plus "All songs") → tracks. Albums → tracks.
  Genres → artists → albums → tracks. Playlists → `.m3u/.m3u8` files found on
  the card plus built-in "Favourites" and "Queue".
* Before the library index exists (first use, or card changed), the tag-based
  entries show "Building library… n files" with a progress row, while
  **Folders and Shuffle all work immediately** (folder walk, no index).
* The index is a file on the card (`.orpheus/library.ydb`), built by a background
  task at low priority, incrementally (per-directory signature of entry count,
  sizes and timestamps), pausing while audio buffers are low. Format and RAM
  bounds: 04-audio §"Library index".
* OK on a track plays it and makes the surrounding list the queue (album order
  for album lists, list order otherwise). OK on a container (artist, album,
  folder, playlist) opens it; Menu → Play / Shuffle / Play next / Add to queue
  / Add to playlist applies to tracks and containers alike.

**Now Playing**

```
┌────────────────────────────┐
│ 12:41        ⇄ ↻  ▮▮▮▮▯    │
├────────────────────────────┤
│ ┌──────┐  Track title that │   title: UI font, 2 lines max, marquee
│ │ art  │  scrolls          │   artist, album: dim
│ │56×56 │  Artist           │
│ └──────┘  Album            │
│  FLAC 44.1/16      7 / 12  │   format badge · position in queue
│ ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▭▭▭▭▭▭▭▭▭▭▭ │   progress
│ 1:47               -2:05 ▶ │
└────────────────────────────┘
```

* Views (Up/Down cycle; the choice is remembered): Standard (above) · Big art
  (full screen, text overlay for 3 s after a key) · Lyrics (synchronised `.lrc`
  beside the file or embedded unsynchronised lyrics, current line centred and
  highlighted) · Up next (queue list; OK jumps; Menu removes/moves) · Details
  (codec, bitrate, sample rate, channels, ReplayGain, file path, size,
  decoder CPU %, buffer state) · Levels (stereo peak meters and a 16-band
  spectrum; drawn at ≤ 15 fps and only while visible).
* Keys: OK play/pause (60 ms fade); Prev tap: restart the track if more than
  3 s in, else previous; Next tap: next; Prev/Next hold: seek, accelerating
  from 2 s/step to 30 s/step, audible scrubbing off by default; OK hold: stop
  and clear the resume point.
* Menu: Shuffle (off/on) · Repeat (off/one/all) · Add to Favourites · Add to
  playlist… · Bookmark here · A–B repeat (set A, set B, clear) · Speed (0.5× …
  2.0×, pitch kept) · Equaliser… · Sleep timer… · ~~Output
  (Headphones/Bluetooth)~~ (hidden: no route to choose, §6.4) · Rate? (not in v1) · Go to album / artist / folder · Delete file… · Details.
* Behaviour: gapless between consecutive tracks of the same format family;
  ReplayGain per settings; automatic bookmarks for files longer than 20 min
  (position saved every 30 s and on pause/skip; resuming such a file asks
  nothing, it just continues); resume across power cycles to within 1 s.

**Equaliser screen**: preset list (Flat, Rock, Pop, Jazz, Classical,
Electronic, Hip-hop, Bass boost, Vocal, Custom 1–2) and a 5-band editor
(60 Hz, 230 Hz, 910 Hz, 3.6 kHz, 14 kHz; ±12 dB in 1 dB steps; Prev/Next select
band, Up/Down change gain; pre-amp row avoids clipping automatically).

**Settings owned by Music** (also reachable from Settings → Sound/Playback):
shuffle, repeat, gapless, ReplayGain (off/track/album/auto, pre-amp), fade on
pause/skip, seek step, speed, EQ, balance, bass, crossfeed, channel mode
(stereo/mono/swap), volume limit, resume on start-up, tag code page.

**As built (VER-13, 2026-11-26).**

* **Seek step** (0x0205: 2, 5, 10, 30 s; 5 s by default) is the first rung
  of the held-key ladder 2 / 5 / 10 / 30 s: the first steps of Prev/Next
  held are the setting's, one rung up after 6, 12 and 20 repeats, at most
  30 s (`orpheus_apps::screens::player::seek_step_ms`; "2 s" is the ladder
  above from its start).
* **Fade on pause** (Settings › Sound, 0x0109, on by default): a pause
  fades the output to silence over 60 ms (the decoder runs on until the
  chain's fade is done, the faded end plays out, then the DAC stops);
  playing again fades in over 60 ms. Off: the DAC stops at once, playing
  again is at full level. The fade around a skip (05 §7.1 "fade on
  pause/skip") is not built: a skip stops and restarts the output.
* **Gapless** (0x0203, on): off, nothing is read ahead and each track that
  ends is followed by a fresh start (the PCM ring plays out, the DAC
  stops, the next file opens: a short silence). The decoders still trim
  their encoder delay and padding.
* **Auto-bookmark long files** (0x0207, on): a file longer than 20 min
  keeps its place (`orpheus_player::AutoMarks`, the last three, in
  `player.bin` after the bookmarks) when it is paused, left (Next, Prev, a
  jump, a new queue, a bookmark, Stop), every 30 s of play and at
  shutdown; opened again from its start it continues there; played to its
  end it forgets the place. The 30-second places reach the card at the
  next save (pause, stop, track change, power-off), not while playing.
  They are not shown in Bookmarks.
* **Speed** (Now Playing's menu, 0x0206) has no effect: 04 §5.1 specifies a
  pitch-kept time stretch (WSOLA, 4 KiB of state, < 15 % CPU) that is not
  built, and RAM for it has no home yet (VER-13 entry in
  `tests/settings/effects.toml`). Proposal (owner/lead): take Speed off
  Now Playing's menu until the time-stretch package lands, or mark it
  "not built" there.

**Acceptance**: every list reachable; 3000-entry lists scroll without stalls;
track change by end-of-track and by Next keeps the display consistent; resume
after simulated power loss; all views render with missing tags and missing art;
titles in Cyrillic, Greek, CJK render; **[HIL]** underrun counter stays 0 while
scrolling every list at full repeat rate during 320 kbit/s MP3 and 16/44.1 FLAC
playback.

### 7.2 Files

A browser over the card. Directories first, then files, natural sort
("Track 2" before "Track 10"), case-insensitive. Hidden and system entries
(`.orpheus`, dot-files, `System Volume Information`) hidden unless enabled.

* Row: type icon (folder, audio, playlist, image, text, disk image, firmware,
  other), name, size for files when focused.
* OK: folder → enter; audio → play, queue = this folder from that file;
  playlist → open as list; image → viewer; `.txt/.lrc/.cue/.md/.log` → text
  viewer (word wrap, page by Prev/Next, remembers position); `.iso/.img/.bin`
  → USB Drive app with the file selected; `.yfw` → firmware update prompt;
  `otpauth.txt` → Authenticator import prompt.
* Menu: Play folder · Shuffle folder · Play next · Add to queue · Add to
  playlist… · Mount on USB… (disk images) · Rename… · Delete… · New folder… ·
  Properties (size, dates, tags for audio, dimensions for images) · Sort (name/
  date/size) · Show hidden · Card info (capacity, free, file system).
* Large directories: listing never loads a whole directory into RAM. The
  browser keeps a sorted **window**; the sort order of directories with more
  than 128 entries is kept in a cache file per directory under
  `.orpheus/cache/`, validated by a signature of the directory contents, so
  paging is immediate after the first visit (04-audio §"Directory index").
  First visit shows names in on-disk order immediately and re-sorts when the
  index is ready, keeping the focused item in place.

**Acceptance**: 5000-entry directory opens to a usable list in < 300 ms and is
fully sorted in < 3 s on the reference card; delete and rename update the list
and the queue; long names, Unicode names, 8.3-only names; exFAT and FAT32 cards.

### 7.3 Radio

```
│  FM            ▂▄▆  ST     │   band · signal bars · stereo
│                            │
│      101.90                │   large numerals, MHz
│  P03  Station name / RDS   │   preset number and name; RDS PS if available
│  radio text scrolling …    │
│ ◂ tune ▸   OK mute         │
```

* Prev/Next tap: step one channel (region step 50/100/200 kHz), wrapping at
  the region's limits; hold: seek to the next station (hardware seek, past
  the band edge round to the other end). Down/Up: next/previous preset (in
  list order, wrapping, as every list moves). OK: mute. OK hold: stop the
  radio (powers the tuner down).
* Menu: Save preset / Remove preset · Presets… (list, rename, reorder, delete)
  · Auto-scan (fills up to 30 presets, strongest first, asks before replacing)
  · Record (opens the recorder with FM as the source) · Force mono · Region
  (Europe/US/World 87.5–108, Japan 76–95, wide 65–108; de-emphasis follows) ·
  ~~Output~~ (none: the radio's analogue bypass reaches the headphones and a
  connected Bluetooth sink alike, U-BT-2; §6.4).
* The earphone lead is the antenna. If earphone detection exists and reports
  none, show "Plug in earphones — the cable is the antenna" but still allow
  tuning.
* The radio keeps playing when the app is left (status bar shows FM); starting
  music stops it and vice versa.
* RDS (programme service name, radio text, clock time as an optional time
  source) only if the tuner chip supports it (docs/hw/fm-i2c-imu.md).

**Acceptance**: tune limits and wrap per region; presets persist; seek stops on
simulated stations; **[HIL]** tuner register read-back matches the displayed
frequency; audio present on the output path (level meter on the ADC loop if the
hardware has one).

**As built (UI-5a, 2026-11-24).** Screens `Radio` and `RadioPresets`
(`orpheus-apps/src/screens/radio.rs`) over `RadioService`; opening the app
starts the radio at its last frequency (music stops). Top line: FM, three
signal bars, ST, the preset number; the caption under the frequency is the
state or the RDS name; the text line is the radio text (scrolling) or, with
no earphones, the antenna note. While seeking or scanning a Prev/Next tap
(and Back while scanning) stops it. Presets: 30, named from RDS, listed in
order (Move up/down, Remove). Auto-scan asks only when presets exist; its
stations replace them strongest first and a toast gives the count. Region
Wide is 65–108 MHz (tuner limit). RDS clock time (group 4A) sets the clock
when Settings › Date & time › Set from RDS is on (VER-13, 2026-11-26).
Not built: preset rename (no text entry yet), the menu's Record item (the
Recorder records FM with Source = FM radio), Output (none to choose,
§6.4). (Checked against the code 2026-11-27, VER-13: Down is the next
preset, Up the previous; steps and seeks wrap at the region's limits.)

### 7.4 Bluetooth

The fitted module is an audio transmitter (06 §4): this app sends audio out.
There is no receiver ("speaker") mode. The presenter remote and pointer are
their own app (§7.12) and share the pairing screens below.

* Top: Bluetooth on/off toggle; device name (editable).
* **Listen** (send audio out): paired audio devices with state (connected,
  streaming, codec); OK connects/disconnects; "Pair new device…" scans and
  lists by name with signal strength, OK pairs (PIN 0000/1234 tried, numeric
  comparison confirmed with OK).
* Menu on a device: Connect/Disconnect · Forget · Use for audio automatically ·
  Details (address, profiles, codec, bitrate).
* Auto-reconnect to the last audio device at power-on and when Bluetooth is
  switched on (limits as in the stock configuration: bounded retries, then
  idle).

**Acceptance (simulator)**: all states with a scripted fake stack. **[HIL]**
per role, defined in 06.

### 7.5 Recorder

```
│ ● 00:12:47.3      MIC      │   state · elapsed (large) · source
│ L ▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯  -9 dB   │   meters with peak hold; clip turns red
│ R ▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯          │
│ WAV 44.1 kHz 16-bit mono   │
│ 5 h 12 min left            │
│ OK pause · Next mark       │
```

* Idle: shows the armed source and format, live meters (monitoring), remaining
  time. OK starts; while recording OK pauses/resumes; Next drops a marker (cue
  point in the file and a new file if "Split on mark" is on); OK hold or Back
  asks "Stop and save?" (Back again discards nothing — recordings are always
  saved).
* Up/Down: input gain (manual), shown in dB with the meter responding live.
* Menu: Source (Microphone / FM radio; the YM03 has no line input) ·
  Format (WAV PCM · WAV ADPCM · FLAC · MP3 · Opus voice — those built, see 04)
  · Quality (sample rate, channels, bitrate) · Gain mode (manual / automatic) ·
  Voice activation (threshold, hang time) · Auto split (off, 15/30/60 min,
  2 GiB) · Pre-record 2 s · Monitor through headphones · Low-cut filter ·
  Recordings… (list in `/Recordings`, newest first, OK plays, Menu deletes/
  renames).
* Files: `/Recordings/REC-YYYYMMDD-HHMMSS.ext`, FM: `/Recordings/
  FM-101.90-YYYYMMDD-HHMMSS.ext` (one folder; `-2`… for a second file in the
  same second; `REC-0001.ext`… while the clock is not set). The header is
  rewritten every 10 s so a power loss costs at most 10 s.
* The key click is forced off while the microphone is live. Playback of other
  audio is stopped when recording starts.

**As built (UI-5b, 2026-11-24)**: the app captures for the meters while it
is shown (it stops 3 s after it is left); OK record / pause / resume, Next
mark, OK held or Back asks "Stop recording?" (Stop focused: nothing is lost);
Up/Down the manual input gain (−12…+12 dB from the source's default; a toast
with the value; with automatic gain on they say so). Menu while ready:
Source, Format (WAV, WAV ADPCM, FLAC), Quality (16 / 44.1 / 48 kHz; mono from
the microphone, stereo from FM), Automatic gain, Voice
activation, Auto split, Monitor, Low-cut, Recordings…; while recording: Stop,
Automatic gain, Voice activation, Low-cut. Recordings… is the Files browser
on `/Recordings`, newest first (by date), audio only: OK plays, Menu → Play
folder, Properties, Delete… (with its question; the only delete the firmware
allows outside `/.orpheus`). Choosing FM starts the radio. Line-in was
offered where a board had one and is retired (2026-11-27, VER-13): the
YM03 has none (01 §2, U-AUD-4), so Source is Microphone or FM radio and a
stored Line-in reads as Microphone; the developer command keeps `rec start
line` for probing. Not built: split on mark,
rename, pre-record, the 2 GiB split, monitoring on the device.

**Acceptance**: state machine incl. pause, mark, split, card-full (stops and
saves cleanly at 1 MiB free) and card-removed; file validity checked with
`ffprobe` in host tests; **[HIL]** recorded FM or loopback tone has the expected
length (± 0.1 %) and no dropped blocks (encoder block counter contiguous).

### 7.6 Pictures

* Browser: 3 × 3 thumbnail grid (40 × 30 cells) with a name line in the strip,
  or list mode; thumbnails are generated lazily (JPEG DC-only decode is used
  for speed), cached under `.orpheus/cache/thumbs/`, and never block navigation.
* Viewer: full screen, fit inside by default. OK cycles Fit → Fill → 100 %
  (pan with the direction keys; Prev/Next at an edge go to the neighbouring
  image). Prev/Next: previous/next image in the folder.
* Menu: Slideshow (interval 2–30 s, shuffle, repeat, with music continuing) ·
  Rotate 90° (remembered per file in the cache; EXIF orientation honoured) ·
  Set as wallpaper (Home and lock screen, stored as a 128 × 128 RGB565 file) ·
  Details · Delete….
* Formats: JPEG baseline (any size, streamed and box-downscaled); progressive
  JPEG shown from its DC and first AC scans at reduced detail (noted in
  Details); PNG (non-interlaced any size; interlaced up to 1024 px wide); BMP;
  GIF (first frame; animation when the logical screen is ≤ 256 × 256); QOI.
  Limits come from the memory plan in 04-audio §"Picture decoding".

**Acceptance**: golden-image tests for each format/scale mode against a host
reference downscale (PSNR ≥ 30 dB); corrupt files produce the error toast, not
a panic (fuzzed on host).

### 7.7 Steps (pedometer)

```
│        6 482               │   today's steps, large numerals
│  ▰▰▰▰▰▰▰▰▱▱▱▱  of 10 000   │   goal bar
│  4.7 km    212 kcal  58min │
│  M  T  W  T  F  S  S       │   last 7 days, bars
│  ▂  ▅  ▇  ▃  ▆  ▁  ▄       │
```

* Tabs: Today · Week · Month (bar charts; OK on a bar shows that day's numbers).
* Menu: Daily goal · Stride length (or "from height") · Weight · Units
  (km/mi) · Count in background (off / while on / always) · Reset today… ·
  Sensitivity (if the software detector is used).
* Counting runs in a sensor task independent of the app. "Always" keeps
  counting in standby if the sensor can count or wake the SoC on its own; the
  capability and the algorithm are specified in 03-hal-drivers §"Motion sensor"
  after docs/hw/fm-i2c-imu.md. Daily totals are stored at midnight and at
  shutdown (settings store, 90 days kept).
* Distance = steps × stride; energy = distance × weight × 1.036 kcal/(kg·km)
  walking coefficient (documented in the app's Details row, not presented as
  medical data).

**Acceptance**: recorded accelerometer traces (walking, running, bus ride,
device on a table) replayed in host tests give step counts within ± 5 % of the
labelled truth and zero steps at rest; day rollover; history persistence.

**As built (UI-5c, 2026-11-24)**: tabs Today · Week (Prev/Next). Today:
the count in large numerals, "Goal 10 000" (or "Goal reached"), the goal
bar, distance and kcal. Week: seven bars (oldest left) with a dotted goal
line and weekday names; Up/Down choose a day, its count and distance show
below. Menu: Daily goal (1,000–30,000 by 500), Stride length (30–150 cm),
Weight (30–200 kg), Units (km/mi), Count in background (Off / While on;
Off counts only while the app is open), Sensitivity, Reset today (asks).
Settings keys `0x0D20–0x0D25`. History: today and six days in
`/.orpheus/steps.bin` (versioned, CRC-checked), written 10 min after the
first unsaved step, at a day change, at power-off and before a card
hand-over; not kept while the clock is not valid (the strip says so).
Not built: the Month tab (needs 30 days of history), "Always" (needs
PWR-3c), the Details row with the coefficient, active minutes.

### 7.8 Stopwatch

Tabs: **Stopwatch · Timer · Alarm**.

* Stopwatch: `MM:SS.cc` (hours appear past 59:59), OK start/stop, Next lap
  (while running) or reset (while stopped, after confirmation if laps exist),
  Up/Down scroll laps (lap time and split). Time comes from hardware timestamps
  taken at the key edge, not from UI frames. It keeps running when the app is
  left, with the screen off, and across a restart (start instant stored against
  the RTC).
* Timer: set with the picker (h:m:s), OK start/pause, Next reset; presets for
  the last five durations; at zero: alarm sound through the headphones (ducking
  the music), screen on, toast; works from standby through the RTC alarm.
* Alarm: up to 4 alarms (time, repeat days, action: beep / resume music /
  radio preset, volume ramp). Requires the RTC alarm wake-up (03 §"RTC"); if
  the hardware cannot wake from power-off, alarms work from standby only and
  the UI says so.

**Acceptance**: timing accuracy against the host clock over 10 min within
± 0.05 % **[HIL]**; lap arithmetic; persistence across restart.

**As built (UI-5d, 2026-11-24)**: one screen with tabs Stopwatch · Timer ·
Alarm (only the active name when they do not fit). Prev: previous tab.
Next: lap (running) / reset (stopped, confirmation if laps exist) / next
tab (idle) on the Stopwatch; reset (started) / next tab on the Timer; next
tab on Alarm. OK: start/stop, start/pause, alarm on/off. The stopwatch is
timed from the key's down edge (the input task's 10 ms scan, the 1 MHz
timer): `MM:SS.cc`, `H:MM:SS` from an hour; 20 laps kept (number, lap,
split; Up/Down scroll). Timer: five presets as chips (the last five
durations, settings `0x0D40–0x0D44`, newest first), Up/Down choose one,
Menu › Set duration (h:m:s picker, up to 9 h). At zero: the alert screen
over any screen, the screen lit, a 2 kHz beep pattern through the
headphones (the music pauses and resumes after), the backlight flashing,
for a minute or until OK. Alarm: four alarms (time, repeat days or once,
on), Menu › Time… / Repeat…, settings `0x0D30–0x0D3B`; Next on the alert
snoozes 9 min; a one-time alarm switches itself off. Status bar: bell
while an alarm is on, a timer runs or an alarm is snoozed; dot while the
stopwatch runs. Auto-off waits while the stopwatch or the timer runs.
Not built: the alarm actions resume music / radio preset and the volume
ramp (beep only), persistence of the stopwatch across a restart, the
alarm wake from off (03 §11, PWR-3c: the Alarm tab says "Rings only while
on").

### 7.9 Authenticator

Time-based and counter-based one-time codes (RFC 6238 / RFC 4226); details and
threat model in 06-connectivity §"Authenticator".

* Locked state: PIN pad (4–8 digits) or passphrase; wrong entries are delayed
  (1 s doubling to 5 min). The vault relocks on screen-off and after 60 s idle.
* List: issuer and account per row, the current code in 16 px numerals at the
  right, a shrinking bar for the remaining seconds (red in the last 5 s).
  OK on a row: big-code screen (28 px numerals, grouped 3 + 3) with countdown;
  OK again: **type the code** through the USB keyboard
  personality followed by Enter if enabled.
* Menu: Add… (import from `/.orpheus/otp-import.txt` on the card — the file
  is overwritten and deleted after import; or manual entry: issuer, account, Base32 secret
  with the Base32 keyboard, digits 6/8, period 30/60, SHA-1/256/512) · Rename
  · Delete… · Move · Next code (HOTP) · Time check (shows RTC time, UTC offset,
  last synchronisation and estimated drift; warns if the clock was never set)
  · Change PIN · Wipe vault…. *As built:* Time check shows the date and
  time in UTC the codes are computed from (the clock keeps local time; less
  the Time zone setting), labelled "UTC" (it showed the local time with
  that label until 2026-11-27, `regress_auth_time_check_shows_utc`);
  offset, synchronisation and drift are not shown.
* Codes depend on correct UTC. The clock can be set from the host tool, from
  RDS clock time, or by hand; the app shows a warning strip if the RTC lost
  power since the last set.

**Acceptance**: RFC 6238 Appendix B and RFC 4226 Appendix D vectors; PIN
lockout timing; import parser fuzzed; codes roll over on the second.

### 7.10 USB Drive

Present a file from the card to a computer as a disk or a CD-ROM, for booting
installers and rescue systems (06-connectivity §"Disk and CD emulation").

* List of `.iso` and `.img` (and `.bin/.raw/.vhd` fixed) files found under
  `/Images` and the last-used locations, plus "Choose file…".
* After choosing: Mode (CD-ROM for `.iso`, Removable disk otherwise; user can
  override) · Read-only (forced for CD-ROM) · Start. The player then
  re-attaches to the host in that personality and shows the file name, mode,
  bytes read/written and current rate. Back asks to stop (warns if the host
  has written data that is still being flushed).
* "Start with this image when plugged in while holding Next" option for use as
  a boot stick without navigating.
* Fragmented files are supported up to the extent-table limit; beyond it the
  app explains ("file is too fragmented; copy it to the card again").

**Acceptance (host tests)**: SCSI command set against a scripted initiator;
**[HIL]** Linux host reads the whole image back and the SHA-256 matches; an
ISO is detected as an optical drive with correct TOC; user-assisted check list
for BIOS/UEFI boot on real PCs.

### 7.11 Settings

A list of groups; every setting applies immediately and is stored by the
settings service (debounced write).

| Group | Items |
|---|---|
| Sound | Equaliser · Balance · Bass · Crossfeed · Channel mode · ReplayGain · Volume limit · Key click · Fade on pause |
| Playback | Shuffle · Repeat · Gapless · Resume at start-up · Seek step · Speed · Auto-bookmark long files · Tag code page |
| Display | Brightness · Backlight time-out (5 s … always) · Screen-off lock delay · Theme · Accent · Wallpaper · Screen saver (off / clock) · Animations · Rotation 180° · Clock in status bar · **List roll-around (on / off)** · Home screen (order, hide) |
| Power | Mode (Automatic / Battery saver / High performance: the clock, 02 §5.4; `docs/user/power.md`) · Sleep timer · Auto power-off when idle (off, 5 … 60 min) · Keys when locked (none / volume / volume + transport) · Battery (percent, voltage, state, cycles if tracked) |
| Date and time | Set time · Set date · 24-hour clock · Date format · UTC offset · Correction (s/day) · Set from radio (RDS) |
| Bluetooth | Same content as the Bluetooth app's settings rows |
| Radio | Region · Force mono · Seek sensitivity |
| Recording | Default source · Format · Quality · Gain mode · Folder |
| USB | When connected (Ask / Charge only / File transfer / Keyboard / last image) · Read-only card reader · Developer channel (on in development builds) |
| Language | English · (packs found on the card) |
| Storage | Card info · Rebuild library · Clear caches · Check card · Format card… |
| System | About (version, build id, serial, hardware, licences) · Update firmware… · Diagnostics · Start original firmware (dual-boot layout) · Factory reset… · Recovery mode · Developer (hidden until OK is pressed five times in About: battery logger, PWR-5) |

*As built (VER-13 follow-up, 2026-11-26):* Display › Brightness offers the
stock firmware's levels 1–11, default 6, and writes the stock PWM4 duty of
the level at once as the slider (or the quick panel's row) moves
(01-hardware §9 U-PWM-3; `orpheus_power::backlight`); store key `0x030D`
(the earlier `0x0301`, 1–16, did nothing and is retired, so the player
starts at 6). Timer and alarm alerts flash the backlight at 15 / 3, the
stock maximum, whatever the level. There is no dim state before the
screen goes off (not asked for here; the stock firmware's is duty 2).

*As built (VER-13, DOC-2 findings, 2026-11-27):* **Storage** › Clear
caches, Check card and Format card do their work in the card task, which
stops playback and saves its state first as for a USB hand-over, then
picks the card up again; one at a time ("The card is busy"); a toast
"Started", then the result:
* **Clear caches** (confirm: Clear focused) removes `/.orpheus/cache`
  (thumbnails, album art, the books' indexes and conversions) and
  `/.orpheus/index` (folder indexes): what the player makes again from the
  card's files. Never the library, the queue, saves, bookmarks or anything
  outside `/.orpheus` (the volume's write prefix; CLAUDE.md rule 5).
  "Done", or "The card could not be read".
* **Check card** (no question: it writes nothing) reads every folder (8
  levels below the root; deeper ones are counted, not read) and follows
  every file's cluster chain to its size (`orpheus_fs::Volume::check`).
  Spec silent on the details; chosen: the simplest read-only check, no
  repair. A dialog: "No problems found" with the folders and files read;
  "Problems found" with the count and "Check the card on a computer."; a
  request for the card task (Files, a track, a developer command, and
  also the step counter's or the battery logger's card writes) stops it
  ("Check stopped").
* **Format card…** (destructive dialog, Cancel focused: "Format the card?
  All music and files on the card are erased.") makes a new file system on
  the card's own partition (`orpheus_fs::format`; the partition table is
  never written; a card with no FAT/exFAT partition is refused): FAT32 up
  to 32 GiB, exFAT above and below 33 MiB, label ORPHEUS, the exFAT
  up-case table mkfs.exfat writes. Quick: structures only. Only the card:
  settings and the Authenticator's vault are in NOR and stay. "Card
  formatted" or "Format failed". The owner allowed it on 2026-11-27 (a
  card that is safe to format; relayed by the lead); no automatic test
  formats the player's card.

**System › Diagnostics** is a read-only page (the spec's full list below
is not built): Uptime, Processor (the core clock now, MHz), Code loads
(pager line loads since start-up), Audio underruns, Free stack (bytes
never used). Read when the page opens and on each service event.

Diagnostics (also the bring-up aid for workers): key test (shows raw ladder
values and decoded keys), display test (colour bars, gradients, pixel grid),
audio test (1 kHz tone left/right, sweep), sensor live view, card speed test
(sequential read/write MB/s, random read latency), memory and pager statistics
(free pool, stack high-water mark, faults/s, resident size), clock and power
state, event list (the last 32 coded events: boot reasons, card errors,
underruns, faults survived; 02-architecture §8).

**Acceptance**: every setting round-trips through the store; factory reset
restores defaults and leaves the card untouched; golden images for each group.

### 7.12 Remote (Bluetooth presenter remote; pointer conditional)

Owner's requirement (2026-11-24). Transport: 06 §4.3. The computer sees the
module's Bluetooth keyboard + consumer-control device. **Everything marked
"pointer" below exists only if a Bluetooth transport can carry mouse data
(decision D-10); otherwise it is left out and Up/Down keep their non-pointer
meanings.**

**Screens**

* **Connect**: paired computers (shared list with §7.4, filtered to HID
  hosts), "Pair new computer…" (makes the player discoverable as a HID
  device for 2 minutes, shows the name and the pairing code if the host
  asks for one), and the transport in use. Auto-reconnect to the last
  computer when the app opens.
* **Present** (the main screen, readable at arm's length): large elapsed
  timer or countdown (colour turns amber at the warning time, red at zero),
  slide counter (counts next/previous presses; can be reset), clock,
  connection and battery icons, the active profile, and a one-line key hint.
  The screen stays on at reduced brightness while presenting; timer alerts
  flash the backlight (no vibration motor) and can play a short tone in the
  headphones.
* **Pointer**: live pointer mode, a tilt indicator with the dead zone, and
  click hints; also reachable with a key from Present.
* **Menu**: Profile · Timer (count up / countdown with duration and warning
  time) · Pointer mode (relative / absolute laser / scroll / off) · Pointer
  settings (speed, dead zone, smoothing, invert, clutch hold/toggle) ·
  Calibrate · Black screen / White screen · Disconnect.

**Keys on the Present screen** (the global grammar holds; deviations are
shown in the key hint)

| Key | Tap | Hold |
|---|---|---|
| Next / Prev | next / previous slide | — (no seek) |
| OK | start or resume the show (profile key, e.g. F5 / Shift+F5) | end the show (Esc) |
| Up | pointer clutch: move the pointer while held (relative/absolute modes); in laser mode also switches the presentation tool's laser pointer on (profile key, e.g. Ctrl+L) and off on release | — |
| Down | left click (in pointer modes); black screen toggle otherwise | right click |
| Vol+ / Vol− | volume on the computer (consumer keys), or zoom, per profile | ramp |
| Menu | app menu | quick panel |
| Back | leave the app (asks while a show is running); the link stays up in the background until Disconnect or the auto-off time | Home |

**Profiles** (key maps, editable copies allowed): PowerPoint (Windows),
PowerPoint (macOS), Keynote, Google Slides, LibreOffice Impress, PDF /
browser (arrows, F11), Generic (arrows, B, W, Esc), Media (consumer keys:
play/pause, next/previous, volume), Custom (per-key editor).

**Pointer** (06 §4.3.2): clutch on Up by default; modes relative, absolute
and scroll; Down clicks; double-tap Down double-clicks; hold Down + clutch
drags.

**Power**: sensor at 100 Hz only while the clutch is held or absolute mode
is on; HID reports only on change; target ≥ 8 h of presenting with the
screen dimmed.

**Acceptance (simulator and WASM)**: golden images of every screen and
state; the simulator back end shows what the computer receives (key
names, pointer trail) so the app is testable without hardware; behaviour
tests for each profile's key sequences; **[HIL]** as 06 §4.3.3.

### 7.13 Games (M14; owner's requirement 2026-11-25)

One **Games** app on Home lists five games. They are original
implementations of well-known genres: own names, artwork, sounds and rule
texts, no names, art, levels or look taken from any existing product.

**Common to all games (GAME-0).**
* Game loop on the UI task at up to 30 frames/s with partial screen
  updates; fixed-step logic separate from drawing, so a game is a pure state
  machine (`orpheus-games` crate, host-tested, simulator and WASM site too).
* Back opens the pause menu (Resume, New game, Settings, How to play,
  Quit); any interruption (screen off, power key, alarm, USB, low battery)
  pauses. The game in progress is saved on exit and power-off and offered
  again (`/.orpheus/games/<game>.sav`).
* Best scores and statistics per game and per setting combination
  (`/.orpheus/games/scores.bin`); a name entered once per player.
* Sound effects from a tiny synthesiser (square/noise/sweep voices) mixed
  over the music, which keeps playing (setting: effects on/off, volume,
  music ducking during play). **Not built yet** (2026-11-27): the games
  are silent; the music keeps playing.
* Every game takes a seed: new games are random, replays and tests are
  deterministic. Golden images per screen; key-sequence fuzzing per game
  with invariants (below); `regress_` tests for every bug (CLAUDE.md).
* Settings live in the game's pause menu, are remembered per game, and each
  has a default that suits a first game.

**Blocks (GAME-1)** — falling tetromino puzzle. Well 10 × 20 cells of 6 px
(60 × 120) with next-piece preview, hold slot, level, lines and score
beside it. Keys: Prev/Next move, Up rotates clockwise, OK rotates
anticlockwise, Down soft drop, hold Down hard drop, Menu hold.
Settings: mode (endless, 40 lines, 2-minute score attack), start level,
piece randomiser (bag of seven / pure random), ghost piece, previews
(0–3), hold on/off, lock delay, auto-shift delay and repeat rate, rotation
with wall kicks on/off. Invariants: pieces never overlap the stack or
leave the well; score and line counts match a reference replay.

**Solitaire (GAME-2)** — Klondike. Seven tableau columns of 18 px on
128 px: small card faces (rank + suit glyph, red/black, colour-blind
friendly suit shapes), fanned columns that compress when long. Keys:
Prev/Next move between piles, Up/Down choose depth in a fanned column,
OK picks up and puts down, Menu auto-moves to the foundations, hold OK
undo. Settings: draw one or three, passes through the stock (unlimited,
three, one), scoring (standard, cumulative "casino", none), auto-move to
foundations, auto-complete, timer shown, deal only winnable games (seeded
from a solver check, best effort). Invariants: 52 distinct cards always;
every move legal; undo restores the exact state.

**4096 (GAME-3)** — sliding number tiles. Board 4 × 4 by default; the four
direction keys (Up, Down, Prev, Next) slide, with short animations.
Settings: board 4 × 4, 5 × 5 or 6 × 6; goal tile (2048, 4096, 8192,
endless); chance of a 4 instead of a 2; undo steps (0–3); animations on/off.
Invariants: the sum of tiles grows only by the spawned tile; merges at most
once per tile per move; game over exactly when no move changes the board.

**Artillery (GAME-4, GAME-5; working name "Salvo")** — turn-based artillery
duel, 2–4 tanks, each human (taking turns passing the player, with a "pass
to Player n" screen between turns) or computer.
* **Terrain**: generated per round from the seed; styles rolling hills,
  mountains, canyon, islands over water, plateaus; roughness and height
  range settings; world 128–384 px wide with a camera that follows the
  shell and an overview zoom. Destructible: craters, dirt falling into
  holes, tanks sliding and taking fall damage.
* **Physics** (fixed step, f32 without fused multiply-add so the host and
  the player agree): gravity in m/s² with presets (Moon 1.62, Mars 3.71,
  Earth 9.81, Jupiter 24.8) mapped to a world scale; wind as a horizontal
  air velocity, re-drawn each turn within the set maximum, optionally with
  gusts during flight; quadratic air drag on the shell relative to the air
  (so wind acts through drag, as it does), shell mass per weapon; explosions
  with radius and falloff; trajectories integrated (semi-implicit Euler at
  ≥ 200 Hz) and checked against analytic solutions in host tests.
* **Weapons** (own set): standard shell, heavy shell, cluster, roller,
  digger, dirt bomb, smoke (wind indicator); optional between-round shop
  with earned credits.
* **Computer players**: aim by simulating candidate shots with the same
  physics and a noisy estimate of the wind; bracket from their previous
  shot; difficulty sets the noise, the search budget and whether they see
  gusts (Novice … Expert). Never cheat by reading the exact gust sequence
  at Novice–Hard.
* **Keys**: Up/Down angle, Prev/Next power (hold accelerates), OK fire,
  Menu weapon list, hold Next/Prev pans the view.
* **Settings**: players (2–4, human or computer with difficulty), rounds,
  gravity preset or value, wind (none, light, strong, custom maximum),
  gusts on/off, air drag (off, realistic, heavy), terrain style, roughness
  and seed, world width, tank health, fall damage, weapon set and shop,
  aim guide (none / last shot trace / short preview), turn timer.
* Invariants: no NaN or escape through the terrain at any step; energy
  bounded without wind; terrain heights within the world; the same seed and
  inputs give the same round on host and player.

**As built (GAME-0/1/3, 2026-11-25; host and simulator; on the player's
Home since the GAME-2 integration below).**
* Logic: crate `orpheus-games` (no_std, no alloc, no floats,
  `forbid(unsafe_code)`): SplitMix64 `Rng` (state saved with the game),
  `StepClock` (ticks → whole 60 Hz steps, a gap over 250 ms pauses),
  game buttons and key edges (`Input`: press, release, long), the save
  envelope `"OGS1" | kind | version | length | payload | CRC-32`, the
  score table (24 records of game × variant: best, its detail, played,
  won; the least played record makes room), `Session` (the one game in
  RAM) and `Hub` (session, scores, which games have a save file, the
  pause menu's resume flag).
* App: `orpheus-apps` feature **`games`** (on in the firmware since
  STACK, 2026-11-25, the simulator, the browser build and the UI fuzzer). Screens `Games` (list: a game's value is "Continue" or its
  best under the current settings), `Blocks`, `Game4096`, `GamePause`
  (Resume, New game — asks while a game is in progress —, Settings, How to
  play, Best, Quit), `GameSettings`, `GameHelp`; Games is the Home cell
  before Settings (`App::GRID`; bit 11 of the Home-screen mask, off by default as on the player; the simulator's `set:games=on` sets it). The game
  lives in `GamesService::games`, so any screen may cover it; a game
  screen rebuilt without the pause menu's Resume/New game comes back
  paused ("OK to continue"), as after the screen going off or a tick gap.
  The toolkit's `Wants::menu_key` gives Blocks the Menu key (hold Menu
  still opens the quick panel).
* Files: `/.orpheus/games/blocks.sav`, `4096.sav` (written on pause and
  quit, read when the game is opened, removed when it ends),
  `scores.bin` (written when a game ends).
* Blocks: well 10 × 20 (+2 hidden rows) of 6 px, full screen; panel: up
  to three previews, hold, score, level, lines (n/40), time (40 lines:
  elapsed; 2 minutes: left) or best (endless). Own rules: the piece turns
  in its 3 × 3 box (4 × 4 for the long one, the square does not turn);
  kicks tried in order (0,0), (−1,0), (+1,0), up one, up-left, up-right,
  and ±2 for the long piece; gravity 48 … 1 steps per row for levels
  1–20, level +1 per 10 lines; points 100/250/450/800 × level for 1–4
  rows, +50 × combo × level, soft drop 1 and hard drop 2 per row; lock
  delay restarted by up to 15 shifts or turns per row reached; cleared rows
  flash 12 steps. Down held past the hold threshold hard-drops the piece it
  was pressed on (so a soft drop longer than 0.5 s ends in a hard drop).
* 4096: 4 × 4 / 5 × 5 / 6 × 6 board of 98 px under the status bar; tiles
  a ramp of role mixes; values in the largest face that fits (numerals,
  UI faces, own 3 × 5 digits; "4K" when nothing fits); a 4-frame slide
  animation; OK undoes (the generator restored too), OK on the goal banner
  keeps going.
* Settings (stored, `0x0E__`; `tests/settings/effects.toml`): Blocks mode
  `0x0E01` (Endless), start level `0x0E02` (1–15, 1), pieces `0x0E03`
  (Bag of 7), ghost `0x0E04` (on), previews `0x0E05` (3), hold `0x0E06`
  (on), lock delay `0x0E07` (0.25/0.5/0.75/1 s, 0.5), shift `0x0E08`
  (100–267 ms, 167), repeat `0x0E09` (0–83 ms, 33), wall kicks `0x0E0A`
  (on); 4096 board `0x0E21` (4 × 4), goal `0x0E22` (2048/4096/8192/
  endless, 4096), chance of a 4 `0x0E23` (0/10/25/50 %, 10), undo steps
  `0x0E24` (0–3, 1), animations `0x0E25` (on). Rule settings apply from
  the next game (a toast says so); ghost, previews, shift, repeat and
  animations at once.
* Not built yet: the effects synthesiser and music ducking, player
  names, the artillery game.

**As built (GAME-2 Solitaire and the firmware integration, 2026-11-25;
built, not yet run on the player).**
* Firmware: `orpheus-apps/games` on (with it `orpheus-ui/menu-key`);
  `App::Games.on_player()` true and bit 11 in the default Home mask
  (`settings::HOME_ALL`, every app): Home has twelve cells, Games before
  Settings. A player whose stored mask is the old default (0x07FF) keeps
  it: Settings › Display › Home screen › Games, or `settings default
  0x30b`. The game strings are no longer behind the feature (append only,
  as everywhere).
* `GamesService` on the player: the hub (`GamesRam`, 872 B in `.bss`)
  and a 512 B card buffer; requests become card operations in
  `orpheus_apps::games::GamesIo` (Load, Read, Write, Remove, WriteScores;
  one out at a time, the bytes encoded when it goes out, a game in
  progress written before another is read, a read nobody waits for
  dropped, the score table never written before it was read). The card
  task carries each out as `media::Req::Games` (`fw/firmware/src/games.rs`):
  at once when idle, while music plays in pieces (GAME-4/5: one piece was
  100 ms on the player; `orpheus_apps::games::CardRun`, a path walk with a
  read or a write into the sector cache, one sector written back, the
  sync: the first piece in the request's chore, the rest as `job:games`
  listing pieces under the chore budget; idle, all at once and before a
  pending card hand-over or power-off). `Open` is answered
  late: the hub's `opening` names the game and the game screen shows
  "Loading…" until the file is read (the simulator's `set:gamesio=on` runs
  the same queue one operation per step). A job not started in 3 s is
  taken back; without a mounted card operations fail at once (a read
  starts a new game). Seeds: uptime (µs) and the RTC's seconds, mixed;
  `display_on` from the UI's published screen state (`drain::Shared`).
* Saves: on pause and quit (as on the host), on power-off (before the card
  is flushed, up to 1.5 s) and before the card goes to the computer
  (USB file transfer or a disk image, up to 1 s).
* Solitaire (`orpheus_games::solitaire`): one deck, 13 piles in one array
  of 52 (pile after pile; face-down counts per column), moves as
  `from | to | n | flipped | auto` in a 100-move history; undo takes back
  a move and the moves the game made after it (auto-move,
  auto-complete) exactly. Rules: ace-up foundations by suit, columns down
  in alternating colours, kings (or runs from one) into empty columns,
  stock turns one or three cards, the waste goes back as often as Passes
  allows (unlimited, 3, 1). Safe auto-move: aces, twos, and a card whose
  two lower cards of the other colour are up; also at the deal. Auto-
  complete: stock and waste empty and every card face up → a card every
  4 steps. Game over: won, or no move changes anything (foundation moves,
  waste to column, column moves that turn a card up, empty a column onto
  a card or uncover a card for a foundation; the stock gone through as
  often as allowed turns up no playable card). Scoring (own): standard
  10 per card up, 5 per card turned up and per waste-to-column card, −15
  per card taken back from a foundation, a pass −50 (one card) or −20
  (three), a win +2 per second under 15 minutes, never shown below 0;
  casino: a deal costs 52 from the bank (charged at the deal, so a deal
  cannot be refused for free), 5 per card up, paid when the game ends or
  is left for a new one, the bank a record of the score table (variant
  0xFFFF, `Scores::bank`); none: the best is the time of a won game.
  Winnable deals (best effort): the deal of the seed, else up to 16 deals
  drawn from it, each tried with 4 randomised greedy play-outs of at most
  400 moves (foundations first, then column moves that turn a card up,
  then waste to column, then the stock); measured on the host (release):
  0.9 ms per new game on average, 2.9 ms worst, drawing one; 2.2 / 4.6 ms
  drawing three (75 % of draw-three new games get a checked deal); on the
  player estimated 0.1–0.5 s, the UI waiting.
* Keys: Prev/Next along the piles (stock, waste, the four foundations,
  the seven columns; repeat while held), Up/Down one card more or less of
  a column's face-up run, Up at its top to the pile above, Down from the
  top row to the column below; OK (on release) picks up or puts down (on
  the same pile: back; refused: the target flashes and the cards stay in
  hand; on the stock: draw or turn the waste back); Menu sends the
  cursor's card to its foundation, else every card that can go; hold OK
  undoes; Back puts held cards back, else pauses.
* Screen (full screen, 128 × 128): info line (Score / Bank / Moves; the
  timer m:ss), top row at y 12 (stock, waste — draw three fans the top
  three 9 px apart with the suit under each rank —, foundations), columns
  from y 34, 18 px apart, cards 17 × 20 px; own 3 × 5 rank glyphs and
  5 × 5 / 7 × 7 suit shapes (spade point with stem, heart, three-lobed club
  with gaps, lozenge: they differ by at least three pixels, so suits read
  without colour); red suits in the warning colour, black in the text
  colour; face down: an accent hatch. Columns fan 3 px (face down) and
  8 px (face up), closing to 1 and 5 px when long. Cursor: a 2 px accent
  frame (the cards' border and the gap); held cards: a "good" frame; a
  target mark under the cursor while holding. Partial redraw per pile
  (a changed pile and its row neighbours, which share the gap), the info
  line once a second; plain strokes so a pile drawn over an old frame
  equals one drawn on a clear screen (`solitaire_partial_redraw_equals_full`).
* Settings (`0x0E4_`, `tests/settings/effects.toml`): Draw `0x0E41` (One
  card), Passes `0x0E42` (Unlimited / 3 / 1), Scoring `0x0E43`
  (Standard / Casino / None), Auto-move `0x0E44` (on), Auto-complete
  `0x0E45` (on), Show timer `0x0E46` (on), Winnable only `0x0E47` (on).
  Draw, passes, scoring and winnable apply from the next game; the
  assists and the timer at once. `STORED` 113 → 120.
* Files: `/.orpheus/games/solitaire.sav` beside the others.
* Game loop (host, simulator with every service, per 33 ms frame): Home
  2.9 µs; Blocks falling with a key every 200 ms 7.1 µs and 28.6 dirty
  rows; Solitaire with cursor keys 13.9 µs and 15.2 rows; Solitaire's
  timer alone 3.1 µs and 0.3 rows (`sim/tests/games.rs::game_loop_cost`).

**Brick breaker (GAME-6; owner's request 2026-11-25; working name
"Bricks")** — a paddle-and-ball brick-breaking game in the genre the
owner calls "breakout": an original implementation with its own name,
levels, look and power-ups (no name, layouts or art from Atari's or any
other product).
* **Play**: a paddle along the bottom (Prev/Next move, hold accelerates;
  Up/Down as an alternative), OK launches the ball; the bounce angle
  follows where the ball meets the paddle; bricks in rows at the top, some
  needing several hits, some unbreakable; lives, score, levels; the ball
  speeds up gently over a level.
* **Levels**: an original hand-made set plus generated levels from the
  seed (symmetric patterns, density and hardness rising with the level).
* **Power-ups** (own set, falling from some bricks): wider paddle,
  slower ball, multi-ball (up to 3), sticky paddle, extra life; off as a
  setting.
* **Rendering**: playfield 128 × 116 under a status line, 30–60 fps with
  partial redraw of the ball, paddle and changed bricks only.
* **Physics**: fixed-step, swept collision (the ball never passes through
  a brick or the paddle at any speed: sub-steps sized to the speed), f32
  without FMA or fixed point so replays match on host and player.
* **Settings**: start level, ball speed, paddle size, lives, power-ups
  on/off, level set (hand-made / generated / mixed), paddle sensitivity.
* **Invariants and tests**: the ball is never inside a brick, the paddle
  or a wall after a step; brick counts only fall; score follows the
  bricks broken; deterministic replays; key-sequence fuzzing; golden
  images; settings in the VER-13 manifest with differential tests; saved
  games and scores like the other games.
**As built (GAME-4/5 Salvo, 2026-11-25; host, simulator and firmware:
feature `salvo`, on in the firmware since STACK, see below).**
* Engine `orpheus_games::salvo` (feature `salvo`; the only floats in the
  crate). World: one `u8` height per column, 128/192/256/384 px wide
  (`WIDTHS`), 128 px high (ground at most 104), 4 m per pixel. Terrain per
  round from the seed (or the terrain seed setting and the round): value
  noise of 2–5 octaves (roughness) over a relief range (low 16–48, medium
  12–72, high 8–96 px); hills; mountains (ridged noise); canyon (a gorge of
  quartic profile, steep walls, in the middle fifth); islands (a mound per
  tank over a sea at a quarter of the relief); plateaus (four levels with
  cliffs); mixed (one of them per round). Tanks in random order at evenly
  spread slots with jitter, each on a flat 9-px pad (above the water).
* Physics in SI units, semi-implicit Euler at 200 Hz, 20 steps per 60 Hz
  logic step (game time 6× real time; OK during a flight 4× more):
  `v += a·Δt; x += v·Δt` with `a = (0, −g) − ρ·k·|v − w|·(v − w)`, `k =
  ½·Cd·A/m` per weapon (Cd 0.4), `w` the wind (re-drawn each turn in tenths
  of m/s within the maximum) plus, with gusts, a gust updated every 20
  steps (`0.9·g + U(−1, 1)·0.4·max`, clamped to the maximum) from the game's
  generator. Gravity presets 1.62, 3.71, 9.81, 24.8 m/s²; air 0, 1.225,
  3.675 kg/m³. Only `+ − × ÷` and a Newton square root (`salvo::sqrt`, 4
  steps from a halved exponent): the thumbv7em build has no fused
  multiply-add (checked: 0 `vfma`, 40 unfused `vmla`). Full power is the
  speed (`V_MAX[gravity][air]`, 57–465 m/s) that carries a shell 1.25 widest
  worlds at its best angle (a test checks the table, within 15 %). Hits are
  probed along each step's path at most every half pixel (a shell moves
  under a pixel per step), the firing tank ignored for its first 0.2 s, a
  barrel inside the ground goes off at once. A flight ends after 180 s.
* Weapons (`SPECS`: mass, diameter, blast radius, damage, price, the full
  set's rounds): shell 10 kg/114 mm, r 10, 40 (unlimited); heavy shell
  30 kg/155 mm, r 15, 65, 40 credits, 3; cluster 12 kg, splits at the top
  of its flight into three 1.5-kg bomblets ±18 m/s apart (r 7, 24 each),
  60, 2; roller 10 kg, rolls on the ground (slope acceleration, friction)
  until it stops, meets a tank, a wall over 3 px or 4 s, then r 11, 45, 30,
  2; digger 12 kg, cuts a 24-px trench of 2-px radius along its way, then
  r 6, 22, 25, 2; dirt bomb, a 14-px disc of earth falling onto the
  ground (tanks under it come up), 20, 2; smoke 3 kg, puffs every 0.25 s
  of its flight and three where it lands, drifting with the air for 12 s
  (three puffs at most), 10, 3. Damage `D·(R − d)/R`, `R` the radius plus
  the tank's half width. A blast cuts a disc; each column loses exactly the
  part of the disc below its top (the earth above falls in at once).
* Tanks: 7 × 4 px, fall at 3/16 px per step² when their column drops, a
  fall over 4 px costs 2 health per pixel (setting), slide a pixel every 4
  steps down slopes steeper than 3 px over 2, sink in water. Points: damage
  dealt, 50 per tank destroyed, 100 for the round's survivor; credits the
  same plus 30 a round; the shop between rounds for each human (computers
  buy heavy shells and clusters at once).
* Turns: each round another player starts; a human's turn starts with
  "Pass to Player n" when two or more humans play. Keys: Up/Down the
  barrel ±1° (held: every 3 steps, after 1 s every step), Prev/Next power
  ±0.1 % (held: 1, then 5, then 20 per 2 steps), OK fires on release,
  hold OK the overview (the world squeezed into 128 px), Menu the weapon
  list (Up/Down, OK; Back or Menu closes); during a flight or a
  computer's turn OK is fast (×4) and Prev/Next held pan the view. The
  view follows the shell or the tank whose turn it is; tanks beyond it
  show as their number and an arrow at the edge. Turn timer: the tank
  fires as aimed at 0.
* Computers (`Kind`: Novice, Easy, Hard, Expert): target the nearest tank
  (Novice any); wind estimate = wind + noise (±60/30/12/0 % of the
  maximum) + a bias per round (±40/20/8/0 %); aim point = target +
  correction learnt from the last shot at the same target (`ai_off −=
  miss·4/6/8/10 tenths`: bracketing); search: bisection on the power for
  45°, 60°, 30°, 72°, 20°, 80° (2/3/4/6 angles, 4/6/8/11 steps each; done within
  1 px), shots that would hurt the shooter never chosen; then noise
  ±3/2/1/0° and ±30/15/6/0 ‰ and the barrel turned visibly. Expert with
  gusts simulates with a copy of the game's generator (the gusts the
  flight will draw); below Expert never. One simulated shot per logic
  step, and the screen runs at most one per frame.
* RAM: `Salvo` 680 B (heights 384, four tanks 120, three shells 60, the
  rest), so `Session` 352 → 680 B and the hub 848 → 1,176 B. Built and
  decoded in place (`Session::start_salvo`, `decode_into`): no 680-B copy
  on the UI task's stack. Save `salvo.sav` (version 1): rules, generator,
  phase (a flight saves as settling, a computer's search as starting
  again), heights seven bits each, tanks (≤ 492 B with the envelope; `SAVE_MAX` 512). A pause
  lands a shell in the air first (`settle_now`).
* App: `Game::Salvo` (Games list fourth), screen `Salvo` (full screen,
  `screens/salvo.rs`: two info lines, world, overlays; drawn when its key
  over the state changes). Settings in three groups (the pause menu's
  Settings lists Players, World, Rules; `Game::groups`), 21 settings
  `0x0E61–0x0E76` packed into four stored words `0x0E80–0x0E83`
  (`Setting::pack`: the cache stays within its 128 slots, `STORED` 120 →
  124): players 2/3/4 (2), Player 1–4 Human/Novice/Easy/Hard/Expert
  (Human, Easy, Easy, Easy), rounds 1/3/5/10 (3), turn timer off/15/30/
  60 s (off), terrain (Hills), roughness (medium), height range (medium),
  terrain seed 0–99 (0: new every game), world width (256 px), gravity
  (Earth), wind none/light 4/strong 12/custom (light), custom wind 1–30
  m/s (8), gusts (off), air drag off/realistic/heavy (realistic), tank
  health 50/100/150/200 (100), fall damage (on), weapons shells only/all
  seven (all), shop (on), aim guide none/last shot/preview (last shot;
  the only one at once). Score table: the best human's points, won when a
  human leads; variant from players, hardest computer, gravity, wind,
  gusts, drag, width, weapons, shop.
* Firmware: **built in** (STACK, 2026-11-25). With Salvo the firmware
  measured resident +368 B (constant pools and strings, mergeable
  sections stay resident), `.bss` +368 B (the hub +328, settings +8, …):
  the stack's start moved three 256-B steps, usable 20,480 → 19,712 B,
  margin 3,604 B < 4,096 B. STACK cut the UI task's deepest paths (the
  Authenticator's finishes one future per kind, a code's entry read
  outside the HMAC, the games' saves and score table decoded without
  temporaries on the stack): nested worst case 15,152 B, margin 4,560 B.
  Salvo's own depth on the UI task: `Salvo::step` (with the computers'
  search) 752 B, `SalvoView::draw` 1,192 B.
* Measured (host, release): one simulated shot ≤ 0.43 ms (Moon, 384 px),
  a computer's decision 0.4–2.0 ms (Jupiter fastest, the Moon slowest), 25 ns per physics step, 1 µs per logic step of a
  flight; game loop per 33-ms frame (simulator, all services): aiming
  10.4 µs / 21 dirty rows, a flight 14.6 µs / 38 rows, four computers 27 µs
  / 89 rows. Player estimate (× 40–60, FPU, paged code): a simulated shot
  ≤ 20–25 ms (one per frame), a decision 20–100 ms spread over its frames,
  a flight step ≈ 50 µs.

**As built (GAME-6 Bricks, 2026-11-25; host, simulator and firmware
build, not yet run on the player).**
* Engine `orpheus_games::bricks` (feature `bricks`): **integers only**
  (positions and speeds in 1/256 px, sine and cosine in 1/4096): the
  thumbv7em build of it has no floating-point instruction at all (checked:
  0 `v…` instructions in the Bricks functions, 0 `vfma` in the image).
  Field 128 × 116 px under a 12-px status line; walls left, right, top.
  Bricks 10 × 5 px cells (drawn 9 × 4) in up to 10 rows of 12 from y 12;
  paddle 3 px high at y 106, 16/22/28 px wide; ball 3 × 3.
* Physics: a ball moves at the game's speed in one of seven directions
  each side of the vertical (12°, 20° … 60° from it); each 60 Hz step is
  cut into sub-steps of at most a pixel per axis (as many as the speed
  needs), each sub-step x then y; a move that would overlap a wall, a
  brick or the paddle is not made, the ball turns on that axis instead
  (and hits every brick it would have entered). Every obstacle is thicker
  than a sub-step, so nothing is passed through at any speed (tested at
  5 px/step, twice the fastest the game reaches), and the ball is never
  inside anything after a step. The paddle sets the angle by where it is
  met (centre 12°, ends 60°, to that side) and stops at a ball beside it
  rather than push into it. Speeds 75/98/120 px/s (slow/normal/fast) at
  level 1, +1/40 per level (ten at most), +1/24 of the base every 4 s of
  play up to 1.5 × the level's speed; never above 300 px/s. A ball that
  meets only walls and unbreakable bricks for 15 s is nudged to the next
  angle (no endless loops).
* Bricks: one, two or three hits (10 points per hit it took when new, paid
  when it breaks; fading by wear, a dot per hit left beyond the first),
  unbreakable (grey, riveted); a cleared level +100. Levels: eight
  hand-made symmetric layouts of our own (shelves, funnel, pillars,
  diamond, gates, frame, waves, keep) and generated ones from the seed and
  the level (patterns random / rows / chequer / pyramid / blocks, mirrored;
  4–10 rows, density 45–85 %, harder bricks and from level 4 unbreakable
  ones rising with the level); a breadth-first search over the cells makes
  every breakable brick reachable (walls closing a pocket become three-hit
  bricks), checked for 60 levels × 3 sets × 3 seeds. Level set: hand-made
  (round again with a hit more per brick), generated, mixed (hand-made,
  then generated).
* Power-ups (own set; one breakable brick in seven carries a capsule,
  three falling at most, 0.75 px/step): wider (+8 px a pixel a step, up to
  40, until a life is lost or the level ends; never into a wall or a ball),
  slower (speed × ¾), split (up to three balls), catch (20 s: the paddle
  holds a ball it meets; OK, Menu or 2.5 s lets it go, aimed by where it
  sits), extra life (nine at most). Capsule signs 5 × 3: a bar with ends,
  S, three dots, C, +; colours accent, good, warning and mixes.
* Keys: Prev/Next or Down/Up move the paddle (from 0.75/1.1/1.5 to
  2/3/4.5 px a step over 15 steps held: the steering setting); the last
  pressed wins while both are held; OK or Menu launches; Back pauses. A
  new game opens with the ball on the paddle ("OK to launch"); a game
  continued in mid-rally opens paused.
* Settings (`0x0EA_`, packed into one stored word `0x0EB0`: `STORED` with
  Salvo 124 → 125, without 120 → 121): Start level `0x0EA1` (1–10, 1), Ball
  `0x0EA2` (Slow/Normal/Fast, Normal), Paddle `0x0EA3` (Small/Medium/Large,
  Medium), Lives `0x0EA4` (1/3/5, 3), Power-ups `0x0EA5` (on), Levels
  `0x0EA6` (Hand-made/Generated/Mixed, Mixed), Steering `0x0EA7`
  (Slow/Normal/Fast, Normal; the only one at once). Score table: the
  score, detail the level reached, won once a level is cleared; variant
  from all but steering.
* Drawing (`screens/bricks.rs`): partial redraw: the brick rows whose
  cells changed, and for each ball, capsule and the paddle that moved the
  rectangle over where it was and where it is, each cleared and drawn
  again in order clipped to it (`bricks_partial_redraw_equals_full`); the
  status line (score, "Level n", lives as balls) when it changes; a change
  of phase or view draws the whole screen with its banner (level cleared,
  paused, game over).
* RAM: `Bricks` 256 B (fits under Blocks' and Solitaire's 344: the session
  and hub do not grow); save `bricks.sav` (version 1, all state) 255 B.
  App: `Game::Bricks` (Games list fifth; fourth on a build without Salvo),
  screen `Bricks` (`ScreenId` 55), `GameScreen` keeps ≈ 108 B of drawn
  keys and rectangles for it (the screen enum's size is still the list
  screen's: `.bss` grew by the settings word only).
* Firmware: **built in** (`orpheus-apps/bricks` on in
  `fw/firmware/Cargo.toml`): resident 80,795 → 80,923 B, `.bss` 124,864 →
  124,872 B, slot +16 KiB (two code pages), stack 16,108 B nested worst
  case of 20,224 usable, margin 4,116 B unchanged; `check-resident` ok.
* Measured (host, release): 80 ns per logic step (autopilot play, fast);
  game loop per 33-ms frame (simulator, all services) in play 3.6 µs / 31
  dirty rows (the band from the highest moving thing to the paddle).
  Player estimate (× 40–60): ≈ 5 µs a logic step, ≈ 0.2 ms a frame before
  the flush.

### 7.14 Accessibility and speech (M15; owner's requirement 2026-11-25)

* **Screen reader** (A11Y-1): spoken output from the text-to-speech engine
  (04 §8): screen titles on entry, the focused row with its value and
  position ("Albums, 3 of 12"), changed values, dialogs, toasts, errors,
  the volume, track changes on request. Interruptible: a key stops the
  current utterance. Verbosity (brief, normal, detailed), **speed from
  0.25× to 10× in 0.25× steps** (owner, 2026-11-25; default 1×), pitch,
  speech volume relative to music, music ducking or pausing while speaking,
  key echo, earcons for list ends and roll-around, "where am I" and "time
  and battery" on a hold combination, and a toggle reachable from any
  screen and from the first boot (hold Back and Menu together for 2 s — approved by the owner
  2026-11-27; the original Vol+ and Vol− chord cannot work: they share ladder 1, §1).
  Fully usable with the screen off.
* **Toolkit support**: every widget carries an accessible label and role;
  a host lint test fails for any focusable item without one; each journey
  in the on-device suite also records its spoken sequence as a golden.
* **Vision and motor settings** (A11Y-2): large text (UI font at twice the
  size, lists with fewer rows), high-contrast themes (light and dark),
  reduced motion (no marquee or animations; long names are spoken
  instead), longer hold thresholds and slower repeat, key-press
  confirmation sound.

**As built (A11Y-2, 2026-11-26; host, simulator and firmware).**
Settings › Accessibility (a thirteenth group, keys `0x1001–0x1003` packed
into the word `0x0D60`; ids by high byte `0x10`). Only the settings the
spec names that had no home elsewhere are there: high contrast is two
Themes, reduced motion is Animations off, the confirmation sound is Key
click.
* **Text size** (`0x1001`: Normal, Large, Extra large;
  `orpheus_ui::font::TextSize`, `System::text_size`, `Gfx::text`,
  `Gfx::ui`). Large is the 16 px reading face (DejaVu Sans Condensed,
  `BOOK16`, now always built: 14,468 B of paged rodata; bold drawn
  emboldened); Extra large is the UI font at twice the size (pixel-doubled,
  no rodata; the spec's "UI font at twice the size"). Lists
  (`widgets::list::row_metrics`): rows 19 / 24 / 32 px, five / four /
  three in the content area; section headers in the bold face; with a
  value the label keeps at least two thirds, and at Extra large a row
  whose label and value do not fit draws both in the 16 px face; a key
  pages by the rows the last draw showed (`ListView` remembers them, so
  screens that count normal rows need no change) and the focus is kept
  in view. Menu sheets: fewer rows so the sheet stays ≤ 110 px. The
  strip (hints, mini player) and dialogs use the 16 px face at both large
  sizes (their lines have a fixed height): a dialog's title may take two
  lines, its text up to four, the card over the status bar when it needs
  the height, the last line cut with an ellipsis. Now Playing's Standard
  view drops the art and the codec badge: the title over the whole width
  (the largest face in which no word breaks, two lines, the second cut),
  artist and album in the 16 px face, the queue place, the times in the
  16 px face; its idle view loses the icon. Unchanged: the status bar
  (14 px), big numerals, games, pictures, the reader (its own font size),
  Steps, the Authenticator, the Equaliser, lock and charging screens, and
  lists a screen draws itself (Up next, the Authenticator's codes, alarms).
* **High contrast** (Theme `0x0304` values 3 and 4: "White on black",
  "Black on white"; `Theme::ContrastDark/ContrastLight`, the accent
  setting ignored). Every pairing the toolkit draws text in (text, dim
  text, on-accent, warning, good, a focused row's value, section
  headers, dialog button labels; blends as drawn in RGB565) is at least
  7:1 (WCAG AAA), lowest 7.3:1 and 7.8:1 (`theme::tests::
  high_contrast_themes_reach_7_to_1`; `contrast_table` prints every
  theme). Found on the way: in Dark and Light a focused row's value
  (3.6 / 4.3:1) and a destructive dialog button's unfocused label
  (3.7:1) are below 4.5:1 (not changed here).
* **Reduced motion** = Animations (`0x0308`) off, as before for the
  marquees, now also: the spinner is a still ring
  (`widgets::misc::draw_spinner`), the paused recorder's dot and the alarm
  alert's icon stay lit. There are no screen slides to stop (not built)
  and no blinking text cursor; the screen saver's clock still moves once
  a minute (05 §6.3: against burn-in; a jump, not an animation). Games
  keep their own motion (4096 has its own Animations).
* **Hold delay** (`0x1002`: 0.5, 0.75, 1, 1.5, 2 s) and **Key repeat**
  (`0x1003`: Normal 8 → 30/s, Slow 4 → 10/s, Slower 2/s without
  acceleration): `input::Timing::accessible`, set on the key mapper by the
  firmware's UI loop (`apply_settings`) and the simulator at each key
  edge. Power's menu needs 1.5 s or the delay when longer; power-off stays
  at 4 s.
* **Key confirmation sound**: the spec's key-press sound is Key click
  (05 §3); what a longer hold needs beyond it is to know when a hold
  registered, so Key click (`0x0108`) became a choice: Off, Presses,
  Presses and holds (a second click when a held key reaches its hold
  delay; `System::key_hold_click`, `services::hold_click`, the same
  microphone and radio rules; stored 0/1 keep their meaning).
* Tests: the matrix rows `s1001–s1003` and the dedicated tests in
  `sim/tests/settings_matrix.rs` `effects` (pages by rows shown, holds at
  each delay, repeats, hold clicks, blinks); `input.rs`
  `accessible_hold_and_repeat`; `font.rs` `sized_faces`; `misc.rs`
  `spinner_stands_still_with_reduced_motion`; 63 goldens `a11y_*` (Home,
  Home playing, Files, its menu, Settings, Display, Now Playing, Nothing
  playing, a dialog × large / extra large / high contrast in Latin,
  Cyrillic and Japanese) and `app_group12_*`; suite journey
  `settings_accessibility` (the test look reads the three settings as
  their defaults until a session sets them).

**As built (A11Y-1, 2026-11-26; host and simulator; in the firmware by
default since PG-2, 2026-11-27, with the speech voice (TTS-3): features
`orpheus-ui/screen-reader` and `orpheus-apps/screen-reader`, the
firmware's default feature `speech`).** Module docs (`orpheus_ui::a11y`) are the
reference; in short:
* **Descriptions** (`a11y::Desc`, 536 B on the host): a title, the focused element
  (`Role`: item, submenu, switch, option, check box, button, adjustable,
  tab, cell, key, text; label, value, `State` on/off, selected, checked;
  unavailable; position "3 of 12" among the focusable rows, section
  headers skipped, by row number past 512 rows; pending rows wait), a
  "where am I" summary, the toast shown and the last volume step.
  `Screen::describe` (feature-gated, default empty: the lint refuses it)
  is implemented by every screen of `orpheus-apps` (39 types, 72 screens
  by name); lists take their focus from the `ListView`'s row
  (`Desc::list` over the screen's `ListSource`), so a list screen's
  description is two lines. `Shell::describe` puts the open dialog (title,
  text, focused button) or menu sheet ("Menu" or its title, the item)
  over the screen's (whose title goes in `under`). `AppScreen::describe`
  adds the strip hint to a screen without a focus. Not described: the
  games' boards (title and state only: playing, paused, over), more than
  the first 90 bytes of a text or book page (continuous reading is
  read-aloud's, BOOK-3).
* **The reader** (`a11y::Reader`, 296 B): compares each description
  with the last and says: a new screen or overlay (title, ", menu" /
  ", dialog", the dialog's text or the summary when there is no focus or
  at Detailed, then the focus), a moved focus, a changed value or state
  (only after a key: clocks and meters are said on request), a new toast
  (after the rest), the volume ("Volume 12, maximum"). **Any key press or
  repeat stops the current utterance first** (`SpeechService::stop`, the
  firmware's `Stop::Now`); a screen raised by an event (an alarm) stops
  it too. **Verbosity** (Brief: label, value, on/off; Normal: also the
  role, "unavailable", the position; Detailed: also the summary on entry
  and what OK does). **Earcons** (`a11y::Earcon`, tones as data:
  `Tone { hz: [from, to], ms, level }`): enter (a deeper screen), back,
  menu, dialog, wrap (a list rolled round, also by a page key), end (Up
  or Down moved nothing), on, off, toast, reader on and off. Spoken text
  is English (D-14): role and state words are the reader's, labels the
  UI's strings; `push_speakable` turns the UI's symbols into words or
  pauses (`·` a pause, `×` "times", `−` "minus", `—` alone "none",
  arrows dropped, "Back + OK" "Back and OK").
* **The speech service** (`a11y::SpeechService`: `stop`, `say`,
  `earcon`, `configure`), what the firmware's speech voice will
  implement; `a11y::UtteranceQueue<N>` is the fixed byte ring between
  the reader and the synthesiser (texts ≤ 255 B and earcons; full: the
  newest is dropped). The simulator's (`sim/src/speech.rs`) queues in a
  1 KiB ring and renders with `orpheus_tts::Speaker` at 16 kHz mono in
  simulated time (a stop cuts the audio where a listener hears it cut),
  earcons as sine glides (`orpheus_synth::math::sin_turns`, the same
  samples on every host); it logs every utterance, earcon and stop with
  its time and hashes the whole output (FNV-1a 64).
* **Chords** (`a11y::Chords`, on the raw key edges before the key
  mapper, so they work from any screen, locked, with the screen off and
  with the reader off). **The hardware cannot do Vol+ with Vol−** (both on
  ladder 1, 05 §1: one key per ladder at a time), so the **toggle is Back
  and Menu held 2 s** (one key of each ladder; no screen acts on either
  going down). **Where am I** is a pair held for the hold delay (Settings ›
  Accessibility › Where am I: Back + OK, the default, or Prev + Next, or
  Off): the screen, what it is over, its summary and the focus at
  Detailed verbosity; held on to 2 s, **the time and the battery**. The
  second key of a pair starts the chord: the first is forgotten without a
  release (`KeyMapper::cancel`, `KeyGate::forget`), both releases are
  swallowed. The toggle is also the Settings row (no chord is the only
  way, 05 §1).
* **Screen-off operation** (Keys with screen off, on by default): with the
  reader on, every key works while the screen is off (`KeyGate::
  reader_keys`: no waking press; the screen stays dark) and the backlight
  timeout does not lock the keys (`a11y::Config::timeouts`; Power still
  locks). The simulator says "Keys locked" and "Unlocked" when Power locks
  and unlocks (unlocked: the screen again) and says nothing that changes
  under the lock while the screen is dark (the lock notice ending).
* **Settings › Accessibility** (after A11Y-2's three rows): Screen reader
  (`0x1004`, off), Speech rate (`0x1005`, stored on its own, a range of
  quarters 1–40 shown "0.25×"–"10.0×": the owner's 0.25× steps, default
  1×, on the slider), Voice (`0x1006`: Neutral, Low, Bright), Pitch
  (`0x1007`: Lower, Low, Normal, High, Higher = −4, −2, 0, +2, +4
  semitones), Verbosity (`0x1008`), Sound cues (`0x1009`, the earcons, on),
  Where am I (`0x100A`), Keys with screen off (`0x100B`, on); the six
  small ones packed into the word `0x0D61` (default 2896: every field's
  default). `orpheus_apps::services::reader_config` maps them to
  `a11y::Config`. Strings after the Sequencer's block, behind the feature.
* **Simulator** (`Sim::a11y`, `Host::reader_config` / `set_reader`):
  the reader looks after every key phase and every time step; script
  commands `chord:back+menu[:ms]` and `expect-said:text`.
* **Tests**: `orpheus-ui` unit tests (verbosity, moves, wrap, end,
  values, events queued, pending rows, toasts, dialogs, menus, symbols,
  the queue, chords, the lint, sizes); `sim/tests/screen_reader.rs`:
  **the lint** (93 journeys reach all 72 screens; Down through every
  focus and through each menu: 629 foci, each with a title, a label and
  something to say at every verbosity; the coverage report
  `target/orpheus/a11y-lint.txt` lists what is said on each), **spoken
  journeys as goldens** (`sim/golden/speech/*.txt`: Home → Music →
  Artists → an artist → an album → a song → Now Playing; Settings with a
  sub-page and a choice; the speech rate on its slider; the factory-reset
  dialog; the lock; the chords; keys with the screen off; interrupting;
  each the exact sequence with times, and the audio's hash;
  `ORPHEUS_SPEECH_WAVS=DIR` writes the WAVs), the behaviour (interrupt,
  toggle, where am I and time, lock, earcons) and one differential test
  per setting; the settings matrix rows `s1004–s100b` with a new service
  context `speech` (the reader turned on by its journey on Home: two
  moves and where am I; Language and Home screen now show there too).
* **For the firmware** (TTS-3 / the package that wires speech): a
  `SpeechService` over the speech voice (`Speaker` at 16 kHz while
  nothing else plays, `Stop::Now` on `stop`, the `UtteranceQueue`, earcons
  from `Earcon::tone`, `configure` to `set_speed` / `set_voice(Voice::
  ALL[v].with_pitch(p))`); the UI task calls `Shell::describe` and
  `Reader::observe` after each key phase and service event (and each tick
  while the reader is on), `Chords` on the input task's raw edges before
  the mapper, `KeyGate::reader_keys` and `Config::timeouts` as the
  simulator does; RAM 536 B `Desc` + 296 B `Reader` + 24 B `Chords` +
  the queue (512 B–1 KiB) + `Speaker` 3.4 KB (host sizes); code: the feature's toolkit and app code
  (measured by TTS-3: below). Not done: speech volume relative to music,
  ducking or pausing the music (TTS-3 built them), key echo, track changes said on request away from
  Now Playing, reduced motion's "long names are spoken" (the focus label
  is always spoken whole), the games' boards.

**As built (TTS-3, 2026-11-27; host, simulator, firmware behind its
feature `speech`, off by default: the code window, 04 §8 "As built
(TTS-3)").**
* **Settings › Accessibility** gains **Speech volume** (`0x100C`: Normal,
  −6, −12, −18 dB) and **Music while speaking** (`0x100D`: Lowered,
  Paused, Unchanged), bits 12–13 and 14–15 of the word `0x0D61` (default
  unchanged, 2896); `a11y::Config::{volume, music}`, `services::
  reader_config`. Paused is `services::SpeechPause` (host-tested): the
  player paused while the reader speaks, resumed 400 ms after; music the
  listener starts or resumes during speech plays on under it.
* **The firmware's UI task** (`ui.rs` module `reader`, as `Sim::observe`
  does): the chords on the raw key edges before the key mapper (the
  developer channel's `keys` too), `KeyMapper::cancel`/`KeyGate::forget`
  for a chord's first key, `KeyGate::reader_keys` at every key edge,
  `Config::timeouts` on the screen timers, `Shell::describe` and
  `Reader::observe` after each key phase while the reader is on and once a
  loop (the loop's deadline: the chords', and 100 ms while it speaks), the
  toggle as the setting, where am I, time and battery, Keys locked /
  Unlocked, the pause policy. The test look reads Screen reader as off
  until a session sets it; a session may set `0x1001–0x100D`.
* **Simulator**: the speech volume scales its speech (exact at Normal: the
  goldens' hashes stand), it counts the speech over lowered music
  (`SimSpeech::ducked`, `duck_min`) and runs the pause policy
  (`Host::music_playing`, `Host::speech_step`). A service context
  `speech-music` (an album playing, the reader on, two moves on Home) in
  the settings matrix; dedicated tests `speech_volume_scales_the_speech`,
  `music_while_speaking_lowers_or_pauses`; Settings › Accessibility shows
  13 rows (two goldens' scroll thumbs, the `speech_rate` spoken golden).
* **Not done**: as A11Y-1's list less the speech volume and ducking or
  pausing; the reader in a default firmware build (the owner decides what
  makes room, 04 §8).

### 7.15 Books (reader; M15; owner's requirement 2026-11-25)

* **Formats** (BOOK-1, BOOK-2): plain text (UTF-8, UTF-16 with BOM,
  Windows-1251 and KOI8-R detected), Markdown, FB2, EPUB 2/3 (zip with
  deflate, OPF spine, XHTML reduced to paragraphs, headings, emphasis and
  lists; images skipped in v1). Books live anywhere; a **Books** app lists
  the card's books (title and author from the file) with progress.
* **Reading**: reflowed pages in the UI fonts (sizes small to extra large),
  margins, line spacing, justification, light/dark/sepia palette; Next/Prev
  turn pages, Up/Down scroll by lines, hold Next/Prev chapter, OK the
  reader menu (table of contents, go to percent, bookmarks, font, read
  aloud, auto page turn). Position saved per book; resume on open.
* **Memory**: pages are laid out in a background pass into a page index in
  `/.orpheus/cache/books/`; EPUB and FB2 chapters are extracted once to a
  plain cached form on first open (when no track is loaded, as the inflate
  window shares the decoder arena), so reading during music needs no
  decompression.
* **Read aloud** (BOOK-3): the TTS engine reads continuously from the
  position, highlighting the sentence and turning pages, with the screen
  off, a sleep timer, speed (0.25×–10× in 0.25× steps, its own setting,
  default from the screen reader's) and voice settings, and resume where speech
  stopped.

**As built (BOOK-1, 2026-11-25; host and simulator; in the firmware by
default since BOOK-1/2, 2026-11-27: feature `orpheus-apps/books`, the
firmware's default feature `books`).**
* Core crate `orpheus-books` (`no_std`, no alloc, no `core::fmt`,
  host-tested). A book is converted once, in pieces, to a **normalised
  stream** (UTF-8 text; `0x10 + kind` starts a paragraph of kind body, H1–H3,
  subtitle, quote, verse, preformatted, list item, author line or break;
  `0x08 | flags` sets bold/italic; single spaces, no empty paragraphs) in
  `/.orpheus/cache/books/<id>.txt` with a 32-byte header (source size and
  time, encoding, length, contents count; made again when the book
  changes). Offsets in that stream are the book's positions: they do not
  change with the layout.
* **Encodings** (`enc`): byte-order marks (UTF-8, UTF-16 LE/BE), UTF-16
  without a mark (zero/0x04 bytes on one side), the XML declaration (FB2),
  valid UTF-8, else Windows-1251 or KOI8-R when most letters are high
  bytes (lower case decides: 0xE0–FF in 1251, 0xC0–DF in KOI8-R), else
  Latin-1 decoded as Windows-1252. Streaming decoder, undecodable input
  U+FFFD byte by byte, never stalls.
* **Formats**: plain text: hard-wrapped (lines joined, a blank or indented
  line starts a paragraph) or one paragraph per line (a line over 120
  characters in the head); a one-line paragraph naming a part or chapter
  (Part/Book/Volume/Часть/Книга/Том → H1, Chapter/Prologue/Epilogue/
  Глава/Пролог/… or a Roman or decimal numeral alone → H2) becomes a
  heading, "* * *" a break; `Title:`/`Author:` lines (Gutenberg) for the
  library. Markdown, a reduced set: ATX headings, paragraphs, block quotes,
  bullet/numbered items (flattened), fences (lines preformatted), thematic
  breaks, `*`/`_`/`**`/`__`/`***` emphasis with flanking rules, inline code
  plain, links and images as their text, escapes, HTML tags dropped, YAML
  front matter (title, author); not setext headings, tables, reference
  definitions. FB2: an XML tokenizer (comments, CDATA, PIs, declarations,
  named and numeric entities, namespace prefixes, unclosed and stray tags
  tolerated, 24-deep element stack); sections nest into H1–H3, a
  `<title>` of several paragraphs is one contents entry ("Part One. 1805"),
  subtitle/epigraph/cite/annotation/poem/text-author/empty-line/emphasis/
  strong map to the stream; `<body name="notes">` keeps its title, its
  footnote titles become subtitles (not contents); `<binary>` skipped
  without being kept. EPUB is BOOK-2.
* **Layout** (`layout`): `Liner` breaks at spaces, before/after CJK
  (kinsoku: no line starts with closing punctuation), after hyphens and
  dashes, inside a word only when it alone is too wide (no hyphenation);
  `Pager` stacks lines by pixel height (headings get half a line before,
  a break half a line, paragraph gaps not at a page's top; a page ends
  early rather than span over 1,536 bytes, a line over 256). A line's
  breaks depend only on its start and the stream state there (`Mark`:
  offset, kind, style, first line; one byte in files), so the page index
  pass and the reader drawing a page from its mark find the same lines
  (tests: no word lost or duplicated across all pages, any piece sizes,
  the stable index, fuzzed). Page index `/.orpheus/cache/books/<id>.idx`:
  8 bytes per page, a header with the layout key (geometry, spacing,
  indent and font) and done flag; laid out again when the key changes.
* **Card work** (`shelf::Shelf::step`): one bounded piece per call — the
  library search (16 directory entries, or one 512-byte head read of a
  candidate), one 512-byte read and up to two writes of conversion, one
  512-byte piece of layout (64-byte sub-pieces) or 512 bytes of index
  written, a 2 KiB text-window read, one 64-entry index chunk of a binary
  search, contents rows; folders made one per step, the sector cache
  written back one sector per step (`Volume::flush_step`). Measured on
  FAT32/exFAT images: at most 16 sectors read and 12 written per step (12
  only when a folder is made: its cluster is zeroed). Positions are
  saved when the reader closes, another book opens, or every 16 position
  changes; the state file is rewritten in place (`Volume::rewrite`).
* **Library**: `/Books` and its folders by default, or the whole card
  (Library › Menu › Find books in; hidden entries and `/.orpheus` never),
  8 levels deep, at most 200 books (`.txt .text .md .markdown .fb2`),
  title and author from the file (else the file name), sorted by a 4-letter
  title key (Latin and Cyrillic folded), progress per book (per mille, or
  New); `/.orpheus/books/library.bin` (header, order table, 384-byte
  records: path ≤ 255 bytes, title, author, format, size, time, progress).
* **Reader** (screen `Reader`, 128 × 114): Next/Prev pages (Prev from the
  index, so it also realigns a view moved by lines or a jump), hold
  Next/Prev the next/previous heading, Down one line, Up one line (the
  index page before laid out to the line before the top), OK the reader
  menu (page "n / total · %", Contents, Bookmarks, Add bookmark, Go to,
  Font size, Margins, Spacing, Justify, Palette, Auto page turn, Read aloud
  greyed for BOOK-3), Menu a quick menu (bookmark, contents, bookmarks, go
  to), Back closes (position kept). A position bar at the right edge.
  Contents (`BookContents`: titles indented by level, per cent),
  Bookmarks (`BookBookmarks`: first words, per cent; Menu deletes; 12 per
  book), Go to (`BookGoTo`: per cent, ◀▶ 10 %, ▲▼ 1 %). Pages show as soon
  as the text is converted; jumps by per cent or Prev wait until the
  layout has passed them.
* **Fonts and palettes**: Small = the UI font (real bold), Medium and Large
  = DejaVu Sans Condensed 16 and 20 px reading faces (`orpheus-ui` feature
  `book-fonts`, 35 KB rodata; drawn bold), Extra large = the UI font at 2×;
  italic is drawn slanted (`Gfx::char_styled`), characters missing from a
  face come from the UI font, then the fallback font (CJK 16 px wide).
  Margins 2/5/9 px, spacing tight/normal/relaxed (14/16/19 px at Small),
  justification (breaks unchanged), palette Theme/Light/Dark/Sepia, auto
  page turn off/10/20/30/60 s. Settings 0x0F01–0x0F07 packed in one stored
  word 0x0F80; manifest entries and differential tests (VER-13).
* Home: thirteen apps (Games and Books) make a fourth row that scrolls into
  view; Books is off Home unless the system offers it (the simulator:
  `set:books=on`; `App::Books.on_player()` is false).
* Not done in BOOK-1: the firmware integration (STATUS: RAM, stack and
  what it needs), EPUB (BOOK-2), read aloud (BOOK-3), hyphenation (Extra
  large breaks long words), setext headings and tables, the empty-library
  golden (the simulator's card always has /Books), translations of the
  Books strings.

**As built (BOOK-2, 2026-11-25; host and simulator; in the firmware with
BOOK-1 since 2026-11-27).** EPUB 2 and 3 convert into the same normalised stream and
`.toc` file, so layout, positions, bookmarks and every screen are BOOK-1's.
* **Zip** (`zip`): the end record searched from the file's end back over a
  comment (512-byte reads overlapping by 21 bytes, at most 64 KiB + 22);
  self-extractor stubs (an offset bias) taken; zip64 (fields 0xFFFF…/the
  zip64 locator) and multi-disk archives refused. The central directory is
  walked as a stream for each look-up (nothing of it is kept): entries are
  matched by an FNV-1a 64 hash of their name (ASCII folded, `\` read as
  `/`). Stored and deflated entries only; encrypted (flag bit 0), other
  methods and zip64-sized entries are refused per entry. CRC-32 and the
  declared size are checked at each entry's end.
* **Inflate** (`inflate`): a `no_std` streaming DEFLATE decoder, input in
  any pieces, at most `max` bytes out per call, resumable at any byte. Its
  state is 48 B; the **window (32 KiB) and Huffman tables (2,304 B) are a
  lent scratch** (`inflate::SCRATCH` = 35,072 B) — on the player the
  card task's decoder arena (64 KiB) while no track is loaded (this
  section's "Memory" design). The output is read from the window itself
  (no output buffer). A smaller window (any power of two from 1 KiB)
  decodes streams whose distances stay inside it and refuses the others
  (`Error::Window`); the EPUB job asks for the full one. Caps: an entry
  stops at its declared size (a bomb is cut within one step's 4 KiB),
  parts over 16 MiB and package documents over 4 MiB are not read,
  64 MiB per book in all.
* **Package** (`opf`, over the XML tokenizer split out of FB2 into `xml`:
  names lower-cased without prefix, attribute values with references
  decoded and an FNV-1a hash of the whole value, unquoted values, 204
  named HTML references and numeric ones, `&#128;`–`&#159;` as
  Windows-1252): `META-INF/container.xml` → the package path (if missing
  or naming a missing file: the first `*.opf` in the archive); metadata
  (first `dc:title`, first `dc:creator`, `dc:language`); manifest and
  spine (non-linear items read in place: no text lost). EPUB 3's
  `<nav epub:type="toc">` (nested `ol` → levels 1–3; `span` headings
  without links left out) is the contents, else the NCX (`navMap`,
  `navPoint` nesting, label and `content src` in either order), else the
  headings h1–h3 as for the other formats. hrefs are resolved against
  their document's folder (percent-decoding, `.`/`..`, query dropped,
  links with a scheme ignored).
* **XHTML** (`xhtml`): blocks are paragraphs; `h1` H1, `h2` H2, `h3`–`h6`
  H3 (a heading's lines split by `<br/>` are one contents entry);
  `blockquote`/`dd` quotes, `center` centred, `pre` one paragraph per
  line, `li` items with "•" or the number (`ol start`), `br` a new
  paragraph of the same kind, `hr` a break; `em i cite dfn var` italic,
  `strong b` bold. Not shown: `head`, `script`, `style`, `template`,
  `svg`, `video`, `audio`, `object`, `rt`/`rp`; images skipped without a
  marker; CSS not read. HTML soup: void elements, unclosed `p`/`li`, a
  `head` never closed, stray end tags, nesting deeper than 32 counted.
  Encodings: byte-order mark, the XML declaration, else `<meta charset>`
  or `<meta http-equiv content="…; charset=…">` switches the decoder after
  the tag (the head's detection — UTF-8, else Windows-1251/KOI8-R/Latin-1
  — until then).
* **Contents positions**: before each spine item the job loads the
  contents entries pointing into it (in reading order from where the last
  item's ended; out-of-order contents found by one wrap-around look) into
  a 16-entry watch, refilled as entries resolve. Every element's `id` (and
  `<a name>`) is an anchor; an entry takes the offset of the paragraph
  its anchor is in or starts (always a paragraph byte), its item's start
  without a fragment or when the anchor never comes, and entries never
  reached take their predecessor's offset (a last pass).
* **Card work** (`epub::Epub::step` under `Shelf::step_with`): one
  512-byte read of the book per step at most, a few small scratch-table
  writes (`/.orpheus/cache/books/epub.man|.spn|.tgt`: manifest 16 B,
  spine 4 B, targets 16 B per record; contents patched in place), at most
  4 KiB inflated and parsed. Measured on FAT32 (512-byte clusters) and
  exFAT images: ≤ 16 sectors read, ≤ 12 written per step (as BOOK-1).
  Unreadable parts become a one-line italic note (`[damaged]`,
  `[encrypted]`, `[unsupported]`, `[too large]`) and reading goes on;
  archives that are not zip, zip64, multi-disk, or without a package or
  spine fail to open. The library search reads an EPUB's title and author
  from its package document when the scratch is lent (else the file name,
  replaced by the package's title when the book is first converted).
* **The scratch**: `Shelf::needs_scratch()` says when; `step_with(…,
  Some(scratch))` lends it, unchanged, for every step while it says so. A
  deflated entry without it waits (`Book::waiting`, the reader shows
  "Preparing"); taken away in the middle of an entry, the conversion
  starts again. `Book::lang` (and the cache header) keep the first bytes
  of `dc:language` for read aloud (BOOK-3).
* Files a job leaves (given up when another book opens or the layout
  changes, failed, or the EPUB tables) have their directory entries
  written in the next steps before other work: before BOOK-2 a conversion
  or layout given up lost its clusters on FAT
  (`regress_abandoned_job_loses_no_clusters`).
* Not done: images (and their `alt` text), CSS (`display:none`, page
  breaks), footnote pop-ups (notes are read where the spine has them),
  tables beyond one paragraph per cell, MathML, right-to-left and
  vertical writing, fixed-layout EPUBs (read as reflowable); DRM (Adobe
  ADEPT, LCP: `META-INF/encryption.xml`) is not detected — its encrypted
  parts come out as `[damaged]` or as unreadable characters; font
  obfuscation does not matter (fonts are not read).

**As built (BOOK-1/2 wiring, 2026-11-26): in the firmware behind its
feature `books`, off** — with it the paged code needs 139 of the code
window's 128 pages (Books ≈ 112 KB of code against 33 KB free; numbers and
options in docs/STATUS.md). Everything below is built, host-tested and
checked in a linked image (see STATUS), not run on the player.
* **The split** (`orpheus_books::shelf`): `Shelf` is the screens' model
  (5,476 B: the book with its 2 KiB window, contents and index rows, its
  state; the library's rows and order) and only it changes on a request;
  what a request asks of the card (open a book, write the reading state,
  drop the open book's files or its layout) waits in it for the next step.
  `Desk` is the card side (7,712 B: the job of the moment, the open book's
  files, a 1 KiB I/O buffer and a directory entry). `Desk::step_with(&mut
  Shelf, …)` is one step; `Books` keeps both (simulator, tests). The
  step's buffers are the desk's and every job is built in a function of
  its own: the step future 5,464 → 2,600 B (host), its poll frame on the
  player 10,040 → 2,568 B; a book and the desk are reset in place (a
  `Book` built and moved was 3.8 KB of the UI task's frame twice over).
* **The model between the tasks** (`fw/firmware/src/books.rs`): two statics;
  the UI takes the model at its first use in a UI loop and gives it back
  at the loop's end; the card task takes it for one step at a time — in a
  chore it only tries (a UI loop in progress: the next chore), elsewhere
  it waits for the UI's release; the UI waits for a step in progress
  (≤ one step: the card task preempts it). A data tag (0x102) wakes the UI,
  which turns it into `other::BOOKS`.
* **Card work while a track plays**: chores after the listing pieces, one
  kind per `orpheus_books::shelf::Kind` (`job:bwrite`, `bscan`, `bread`,
  `bconv`, `blay`: each its own measured need), steps while the chore's
  budget lasts; no folder made (`Desk::mkdir` off: a folder that exists is
  looked up, a missing one waits for the idle card task — making one zeroes
  a cluster, 256 sectors on the owner's card); no scratch. **Paused**: steps
  between the paused loop's requests (≤ 150 ms a pass), folders made, no
  scratch (the decoder stays loaded). **No track loaded** (the idle loop):
  steps until other work waits, paced (a rest as long as the step), with
  the decoder arena's first 35,072 B lent while `Desk::needs_scratch`; the
  idle loop counts every pass that may give the arena to something else,
  and a lent step after one is a step without the scratch first (an entry
  half decoded starts again, never from a window someone overwrote).
* **The EPUB hint**: "Stop the music to prepare this book." (`Book::waiting`;
  pausing keeps the decoder in the arena, so it says Stop).
* **USB hand-over** (`Desk::suspend`, `Shelf::card_back`): the reading state
  and the files jobs left are written and the cache flushed before the
  card goes; afterwards the desk starts afresh and the open book opens
  again at its saved position. **Power-off and restart**: the reader
  closes (its position written, ≤ 1 s) before the card task shuts down.
* **Test hooks**: `books status` (the model as the card task left it:
  library and book phase, conversion, waiting, top, page, pages, steps,
  lent steps, retaken scratches, a folder waiting, `busy`: chores that
  found the model held by the UI), `books test on|off`
  (`Scope::Test`: `/.orpheus/test/suite/books`, the suite's books, with a
  library file of their own, `/.orpheus/test/suite/books/.library`, and their
  reading state in RAM only: a test leaves no position or progress behind
  and never rewrites the owner's library; BOOK-1/2b), the
  simulator's `set:bookstest=on`; the suite's `books` journey (skipped on
  an image without the feature: `requires`).
* **Card work while music plays** (BOOK-1/2b, 2026-11-27): an open is a
  step per file operation (the record, the book's entry, its state, the
  cache's header, then the contents file or the book for conversion; a
  conversion makes its text and contents files in steps of their own);
  the clock floor is High while Books has card work and a track plays
  (`audio::books_boost`); a chore piece runs only steps of the kind it was
  admitted for, without card rests (`Urgent`). Measured on the player
  with a 48 kHz, 256 kbit/s MP3: open steps 4–37 ms, 0 underruns over
  four opens, an open in 6–9 s (each step waits for headroom: the chore
  rule's starvation path, `orpheus_audio::chore`).

### 7.16 Sequencer (drum machine and synth; M17; owner's requirement 2026-11-25)

A step sequencer on `orpheus-synth` (04 §9): drum and synth tracks in
patterns, chained into songs.

* **Project**: 16 patterns; each pattern 1–64 steps (16 per page) at
  1/16 notes; 8 drum tracks plus 2 synth tracks (bass and lead, monophonic
  or 4-voice chords); tempo 40–240 BPM, swing, tap tempo; song mode chains
  patterns with repeats. Saved as small files in `/Sequencer/` (or a
  folder the owner picks), autosaved; export of the song to WAV (rendered
  offline, faster than real time where the CPU allows) and to a Standard
  MIDI File.
* **Grid screen**: 16 steps × 8 tracks of 7 × 12 px cells on 128 × 128
  with the playhead, the current track's sound and the pattern number.
  Keys: Prev/Next move along the steps (hold: page), Up/Down change track,
  OK toggles a step, hold OK edits the step (velocity, probability,
  ratchet 1–4, micro-timing, accent, note for synth tracks), Menu the
  pattern and track menu, the transport on a long Menu or from the quick
  panel; Vol± volume as everywhere.
* **Sound screen** per track: drum voice parameters (tune, decay, tone,
  snap/noise, level, pan, send) or synth patch (oscillators, filter, the
  two envelopes, LFO, glide), with presets of our own; per-step parameter
  locks for a few parameters.
* **Synth tracks**: note entry on a one-octave strip with octave shift and
  a scale lock (major, minor, modes, pentatonic, chromatic), live record
  from the keys while playing (quantised), transpose per pattern.
* **Playing**: mute/solo per track, pattern switch at the pattern end,
  fill pattern while a key is held, keeps playing with the screen off; the
  playhead and the audio come from the same sample clock.
* **Settings**: kit and pattern defaults, swing amount, metronome, count-in,
  step length, MIDI clock out over USB MIDI (with M16's USB audio
  personality, optional), key click off while playing.
* **Tests**: golden images per screen; key-sequence fuzzing with
  invariants (steps stay within the pattern, saving and loading reproduce
  the project exactly, render of a saved song is bit-identical); a long
  soak scenario playing a song while editing.

**As built (SEQ-1, 2026-11-26): drum tracks, host and simulator.** New
crate `crates/orpheus-seq` (`no_std`, no `alloc`, no `unsafe`, no
floating point outside the synth); feature `seq` of `orpheus-apps` (on in
the simulator and the browser build; not in the firmware yet: no package
has wired it there, though SYN-3 put the synthesiser on the player).
Module docs are the reference; in short:

* **Model** (`model`): a `Project` of 16 patterns × 1–64 steps × **8 drum
  tracks** (synth tracks are SEQ-2); a `Step` is 3 bytes: on, velocity
  1–127 (kept while off), probability in sixteenths, ratchet 1–4, accent
  (+32 velocity), micro-timing as a **delay** of 0–12/24 step (no early
  push: a step is read when it starts, so every edit is heard at the next
  step; an early push needs a lookahead, SEQ-2 if wanted). Tracks: kit
  note, level 0–100 %, mute, solo (any solo: only soloed, unmuted tracks
  sound). One kit per project (an `orpheus_synth::Kit`, a factory kit by
  name in SEQ-1), all tracks on one engine part so the kit's choke groups
  work across tracks. Tempo 40–240 BPM in 1/100 BPM, swing 50–75 %, play
  mode Pattern (the current pattern loops; another chosen while playing
  starts at the pattern's end) or Song (a chain of ≤ 64 entries of a
  pattern × 1–16 repeats, round again at its end; song editing is SEQ-2).
  Every setter clamps; `rev()` counts changes (autosave, redraws).
  Shortening a pattern keeps its later steps. RAM: project 25,092 B
  (patterns 16 × 1,537), player 1,880 B, studio (project, player, list of
  32 projects) 27,528 B; `reset`/`clear` work in place (assigning the
  `NEW`/`EMPTY` constants put a 25 KB image of them in read-only data).
  Code (thumbv7em, opt "s", no LTO): 15.5 KB with the demo and the
  writer; the play path (`segment`, render, `units_to_frames` with its
  128-bit division per step) ≈ 4.2 KB.
* **Timing** (`player`): units of 1/57,600 of a step pair; a step at local
  index *i* lasts 576·swing (even) or 576·(100 − swing) (odd) units,
  delays 1,200 per 1/24, ratchet hit *j* of *r* at *j*·len/*r* — all whole
  numbers; frame = anchor + ⌊units·rate·5 / (tempo·96)⌋. Every hit on the
  floor of its exact time (tested against an independent rational
  computation at 8/44.1/48 kHz, 40–240 BPM, swing 50–75 %, odd pattern
  lengths, every delay and ratchet, and for an hour without drift). A
  tempo change re-anchors at the next step. Hits are note-ons on part 0
  at their sample; blocks are cut at step starts and when 64 hits are due;
  any block size renders the same samples; the audio equals the engine fed
  the same notes by hand. Probability draws are a hash of the project
  seed, the step count and the track: a saved song renders bit for bit.
* **Screen-off API**: `Studio::render_i16(&mut Engine, &mut [i16])` (or
  `Sequencer::render_*`, `run_dry` for the hits alone) is all the synth
  task calls; the UI only edits the project between blocks and reads
  `Sequencer::position_at(dac_frame)` — the playhead from the audio's own
  clock, less the output latency (16 latched steps remembered).
* **File** (`file`): `/.orpheus/seq/SEQnn.OSQ` (8.3 names, 1–99; CLAUDE.md
  rule 5 instead of the `/Sequencer/` above): `OSQ1`, version 1, records
  (project, tracks, non-empty patterns with their steps past the length,
  song, the kit as an embedded `OSB1` bank record), CRC-32 at the end. A
  new project 383 B, the demo 2,167 B, at most 25,203 B (`MAX_FILE`). Older versions
  load (longer records keep their known prefix, shorter ones keep
  defaults, unknown tags are skipped; `migrate` is the hook); newer ones
  are refused so an older firmware never overwrites them. Fuzz target
  `seq_load` (10 seeds).
* **Screens** (`screens::seq`): *Sequencer* — "New project" (named
  "Beat n", saved at once) and the saved projects (a playing one marked;
  Menu › Delete project). *Grid* (full screen) — header: pattern, the
  track's sound, tempo, play/stop; 8 rows of 16 cells of 7 × 12 px with
  lower-case labels (two capitals of the UI font do not fit 16 px): on
  white, accent in the accent colour, ghost (velocity < 64) grey,
  chance < 100 % hollow, ratchets split; the playhead column lighter;
  footer: pages, step/length and the step's velocity. Keys as above (held
  Prev/Next: by pages), Menu: Play/Stop, Transport, Mixer, Track,
  Next/Previous pattern, Copy to next pattern, Clear pattern, Save; Back
  saves (if changed) and returns to the list; leaving the app stops.
  *Transport* (play, tempo ±1 BPM — held ±10 —, swing, tap tempo (mean of
  the last 4 taps), play mode, pattern, length, kit), *Step* (on,
  velocity, chance, ratchet, delay, accent; hold OK on the grid), *Track*
  (sound among the kit's notes, level, mute, solo), *Mixer* (every track:
  OK mute, hold OK solo, Prev/Next level). Strings English in every
  language until the packs have them.
* **Not in SEQ-1** (SEQ-2): synth tracks, sound screens and parameter
  locks, note entry and scale lock, live record, song editing, WAV/MIDI
  export, fill while held, count-in, metronome, the settings of the
  list above; no new setting was added.

**As built (SEQ-2, 2026-11-26): synth tracks, locks, song, export;
host and simulator.** Not in the firmware yet (as SEQ-1). Module docs of
`orpheus-seq` are the reference; in short:

* **Synth tracks**: tracks 8 and 9 (bass, lead) after the 8 drum tracks.
  A `SynthStep` is 4 bytes: the drum step's on/velocity/probability/
  ratchet/accent/delay (delay 0–12/24 in 4 bits), a note 0–127, a gate of
  ¼–64 steps (16 codes) and a chord (single, triad, seventh — stacked in
  the project's scale —, power). A chord plays once (its ratchet
  ignored); a ratcheted note's gate is cut to the ratchet's length. Each
  synth track has its own engine part with a patch (a factory preset or
  edited: "custom"), level (the part's volume), delay and reverb sends,
  mute and solo (solo is across all ten tracks). Note-offs are computed
  with the note and kept per track (8 sounding notes); an off is on the
  first sample of a render piece before that sample's note-ons (a gate of
  one step re-attacks, a longer one overlaps: a mono legato patch
  glides); Stop ends every synth note at the next render. Pattern
  transpose ±24 (applied before the chord is built); **scale lock**: 10
  scales (chromatic, major, minor, the five other modes, major and minor
  pentatonic) on any root; note entry snaps and walks the scale.
* **Parameter locks**: per pattern a sorted store of 24 locks (4 bytes:
  step, track, parameter, value); a lock beyond is refused (the model's
  `set_lock` returns false, the step screen says "No room for more locks
  in this pattern", nothing changes). Drum tracks lock tune, decay and
  tone (in the kit entry's units; played as the engine part's new drum
  offsets `Param::DrumTune/DrumDecay/DrumTone`, set before the hit and
  back after — bit-identical to the entry changed by hand), synth tracks
  cutoff (log 20 Hz–20 kHz in 128 steps), resonance and glide (the part's
  parameters, the patch's own values back at the next unlocked note).
* **Two kits** per project (A, B; each drum track chooses), kit B on the
  engine's second kit slot and part. **Fill**: a project's fill pattern;
  while the fill key is held its steps play (from the next step, at the
  same position within its length). **Metronome** (a clave on its own
  part, accented each bar) and **count-in** (1–2 bars) are player
  options; **step length** 1/16, 1/8 or 1/32 scales the step clock
  exactly. **Live record**: `Studio::key_note` plays a note now and, when
  recording, writes it into the step nearest the press (the step it falls
  in, the next from its middle on, with swing).
* **File** `OSQ1` **version 2**: new records 6 (pattern tracks of a kind:
  the synth steps with the transpose), 7 (kit B), 8 (a synth track with
  its patch as an `OSB1` bank), 9 (a pattern's locks); the project record
  gains scale, root, fill and drum sends, the track flags the kit; a
  pattern with only synth content writes its drum record with 0 tracks.
  **Migration**: SEQ-1's version-1 files load unchanged (tested against
  the files SEQ-1 saved, `crates/orpheus-seq/tests/data/`), saved again
  as version 2. A new project is 924 B, the demo song 3.6 KB, at most
  `MAX_FILE` (36,041 B; SEQ-1's demo beat is 2,708 B as version 2).
* **Export** (`orpheus_seq::export`): one pass (the song once, or the
  current pattern) with the step length, without metronome or count-in.
  *WAV*: 16-bit stereo at the engine rate rendered offline through its own
  player (`WavExport`, any piece size, the song then 2 s of tails).
  *MIDI*: format 1, 600 ticks a step (2,400 a quarter for 1/16): every
  hit, delay, ratchet and gate on an exact tick; tracks: tempo/name/4/4,
  drums on channel 10, bass and lead on 1 and 2 with a GM program near
  the preset and the level as CC 7. Written to `/.orpheus/seq/SEQnn.WAV`
  and `SEQnn.MID` next to the project (CLAUDE.md rule 5).
* **Screens** (`screens::seq`): the grid shows 8 of the 10 tracks (the
  synth tracks scroll in below; their cells as drums', chords dotted,
  locked steps with a green corner; the footer adds a synth step's note);
  its menu adds Sound, Keys and More › Live, Song, Export WAV, Export
  MIDI (a menu holds 12 items). *Step* adds note, length and chord for
  synth tracks and the three lock rows (◀▶ lock from the track's own
  value, OK clears). *Sound* (drum: tune, decay, tone, snap, level, pan
  of its kit entry, the drum sends; synth: preset, wave, cutoff,
  resonance, filter envelope, ADSR, vibrato, glide, voices 1/4, sends).
  *Keys*: a one-octave strip (theme-independent white and black keys,
  the scale's keys bright), ◀▶ walk the scale, ▲▼ an octave, OK plays and
  enters at the step cursor (held: takes the last note back), Menu ›
  Record: OK while playing writes quantised. *Live*: OK held = fill, ◀▶
  the next pattern, ▲▼ tempo, the count-in counted down. *Song*: the
  entries, ◀▶ pattern, OK one more repeat, Add pattern, Menu › Insert a
  copy, Delete, Play the song. *Transport* adds kit B, scale, root,
  transpose, fill pattern; *Track* adds the drum track's kit; *Mixer* all
  ten tracks. The list's Menu › Settings opens the Sequencer's settings.
  SEQ-2's strings are English and behind the `seq` feature (the
  firmware's stack margin cannot take their read-only data).
* **Settings** (Sequencer › Menu › Settings, word `0x1180`): default kit,
  default pattern length, default swing (of new projects), metronome,
  count-in, step length, key clicks in play (off: "key click off while
  playing"). MIDI clock out over USB is not a setting yet: its model is
  built (below), the transport and the setting wait for M16. With games,
  Books and the Sequencer the settings store was full: 128 of 128 keys
  (VER-4, 2026-11-26: the store's index now holds 192 keys; 131 in use
  with every app, the Bluetooth address, the presets and the retired key).
* **RAM** (`size_of` on the host): project 35,808 B (SEQ-1:
  25,092; a pattern 2,147 B: drum steps 1,536, synth steps 512, locks 97),
  player 3,784 B (SEQ-1: 1,880: a 128-hit queue of 24-byte hits, the
  sounding-note tables), studio 40,144 B, a WAV export's own player
  3,808 B. Kit B costs 289 B + 16 B of names. The engine grew by the drum offsets of its 16 parts: 62,320 → 62,512 B.

**As built (SEQ-2, 2026-11-27): MIDI clock out, the host-side model.**
M16 (the USB audio personality that would carry a USB MIDI interface)
is not built, so only the model is: `orpheus_seq::clock` and
`Sequencer::set_midi_clock` / `take_clock` / `clock_dropped`. **Deferred
to M16**: the USB MIDI streaming interface, sending each message when the
DAC reaches its frame (less the USB latency), and the Sequencer setting
"MIDI clock out" (with its `effects.toml` entry and differential tests
then — a setting with nothing to switch is not added). Where the spec is
silent, the simplest design was chosen:

* **Messages**: Start (`0xFA`) on the first frame of the first step,
  then 24 Clocks (`0xF8`) a quarter note, the first on Start's frame;
  Stop (`0xFC`) on the first frame after a stop, when the clock is
  turned off while running and before a restart (Play while playing:
  Stop, Start). The count-in sends nothing (a receiver starts on the
  downbeat). No Song Position Pointer and no Continue: Orpheus always
  plays from the start of a pattern or the song, which Start means.
  Turning the clock on takes effect at the next play.
* **Timing**: the clock is the tempo, not the steps: 1/24 quarter in the
  player's units (4,800 units for 1/16 steps, 2,400 for 1/8, 9,600 for
  1/32) through the same units → frames map as every step, so clock *k*
  is on start + ⌊*k* · rate · 250 / tempo⌋ (tempo in 1/100 BPM) for every
  step length. With swing 50 every step starts on a clock; swing moves the
  odd steps, not the clock (the receiver swings by itself; after an
  odd-length pattern with swing the steps sit off the clock grid, as the
  player's swing is a step length). A tempo or step-length change keeps
  the clock's phase (the elapsed part of a clock carried in units) and
  takes effect at the step the player changes on.
* **Queue**: 32 messages of 8 bytes (frame, message in the top two bits)
  in the player, taken after every render; a full queue drops and counts,
  the phase goes on. At 240 BPM a block of up to 1/3 s fits.
  `Realtime::usb_packet` gives the USB MIDI 1.0 event packet (code index
  0xF, one byte). Player 3,784 → 4,056 B.

### 7.17 Signal generator (M18; owner's requirement 2026-11-25)

Configurable reference signals on the headphone output (and, if chosen,
over Bluetooth), built on `orpheus-synth`'s band-limited oscillators
(04 §9) and the jack output engine (§7.19 common part).

* **Waveforms**: sine, square, triangle, sawtooth, pulse with duty cycle,
  white and pink noise, linear and logarithmic sweeps (start, stop,
  duration, repeat), stepped tones, multi-tone (up to 8 lines), bursts
  (on/off cycles), impulses, silence; per channel or linked, with phase
  offset and polarity, so left/right identification, phase and polarity
  tests are possible.
* **Parameters**: frequency 1 Hz to 0.45 × the sample rate (fine and
  coarse steps, note names, A4 tuning reference), level in dBFS and, once
  M13 has calibrated the output, in dBV/Vrms into a stated load; DC offset
  if U-AUD-15 shows the output can hold one; sample rate 44.1/48 kHz
  (96 kHz if available); a run timer; presets (1 kHz at −20 dBFS, 440 Hz,
  sweep 20 Hz–20 kHz, pink noise for speaker checks, the M13 test set) and
  user presets.
* **Screen**: a large readout of frequency and level, the waveform drawn,
  Up/Down change the focused value, Prev/Next move between values, OK
  starts/stops, hold Prev/Next fast change; a level ramp at start and stop
  (no clicks) unless "hard edges" is chosen; works with the screen off.
* **Safety**: a confirmation before levels above a set limit with
  headphones (hearing), the volume limit applies unless overridden for
  line-level use; the USB-ground rule for the jack (07 §7) shown once.
* **Tests**: generated buffers checked on the host (frequency, THD,
  level, phase) and through the M13 capture on the player; golden images
  of the screens; VER-13 entries for every parameter.

### 7.18 IR remote (M18; owner's requirement 2026-11-25)

Sends infrared remote-control codes through an **IR adapter in the
headphone jack** (provided by the owner: typically two IR LEDs in
anti-parallel between the left and right contacts, so that opposite-phase
signals on the two channels light one LED or the other).

* **Signal**: the carrier (30–56 kHz; 38 kHz by default) is made as
  anti-phase stereo at half the carrier frequency, so the LED pair sees the
  full carrier at a 48 kHz sample rate (or directly at 96 kHz if the DAC
  allows, U-AUD-16); marks and spaces are gated with sample accuracy; the
  PA runs at the gain the adapter needs (U-AUD-15), volume and DSP
  bypassed while sending; Bluetooth output off for the duration.
* **Protocols**: NEC (and extended), Samsung, Sony SIRC 12/15/20, RC5,
  RC6 (mode 0), Panasonic/Kaseikyo, JVC, Sharp, raw timing lists, Pronto
  hex; repeat frames while a key is held.
* **Remotes**: our own small set of generic layouts (TV, audio, projector,
  air conditioner as raw codes), and the owner's files from the card
  (`/IR/`: a simple text format, Pronto hex, LIRC `.conf` raw and
  space-encoded remotes, CSV of protocol/device/function) — no third-party
  database is bundled unless its licence allows; a "search" mode that steps
  through a protocol's power codes for an unknown device.
* **Screen**: a remote's buttons as a list or a grid, favourites, a macro
  (sequence with delays), the last sent code shown; keys map directly
  (Vol± can send volume codes while the app is open, configurable).
* **Limits**: sending only (no IR receiver on the board); range depends on
  the adapter and the output swing (measured in JACK-0).
* **Tests**: waveform generation per protocol checked on the host against
  the protocol timing specs (bit-exact mark/space lists, carrier duty);
  on the player the jack output captured through the line input (USB
  disconnected) and decoded by a host IR decoder; the owner checks a real
  TV with the adapter.

### 7.19 Peripheral control (M18; owner's requirement 2026-11-25)

Simple control of external devices plugged into the TRS jack through
small adapters, using the two output channels as control signals.

* **Jack output engine (common to §7.17–7.19)**: sample-accurate
  waveforms on L and R independently, the PA at a chosen fixed gain,
  volume, DSP, ReplayGain and the key click bypassed, music playback
  paused, Bluetooth audio off, the output level and limits checked against
  U-AUD-15's measurements; the mode is shown in the status bar while
  active.
* **Signal kinds** (each per channel): a level or gate (DC or, if the
  output cannot hold DC, a tone that an adapter rectifies), pulses and
  pulse trains (period, width, count), servo PWM (50 Hz, 0.5–2.5 ms, for
  hobby servos via a level-shifting adapter), a tone on/off (for a
  tone-detect relay adapter), and serial data over audio (UART-style
  frames at 300–9600 baud or FSK at 1200 baud, for a microcontroller
  adapter), plus control-voltage/gate/trigger for modular synthesisers
  (within the output's range).
* **Devices**: profiles stored on the card (`/Peripherals/*.txt`) naming
  the channel mapping and the controls: buttons (pulse, toggle), sliders
  (level, servo angle, PWM duty), sequences with timing, and serial
  commands; examples shipped for a camera shutter release (via an
  optocoupler), a relay board, a servo, and a modular gate/CV pair.
* **Screen**: the profile's controls as a list (buttons, sliders), the
  current state of each channel, an emergency "all off" on Back-hold;
  works with the screen off for timed sequences (intervalometer).
* **Safety**: adapters, never direct loads (the output is a headphone
  amplifier: no inductive loads, no current beyond its rating); the
  USB-ground rule (07 §7); a confirmation for levels above the headphone
  limit; all outputs return to silence when the app closes, on any
  interruption and at power-off.
* **Tests**: generated control waveforms checked on the host (timing to
  the sample, levels, servo pulse widths, serial frames decoded); on the
  player through the line-input capture; adapter schematics documented in
  `docs/hardware/` (owner-built), each checked once by the owner.

## 8. Non-goals for v1 (recorded so nobody builds them by accident)

Video playback; third-party plugins (games are M14, owner 2026-11-25); touch or pointer UI on the player itself; a
Bluetooth receiver mode; multi-language text shaping
(Indic, Arabic joining); streaming/network features; user theming beyond
palette + wallpaper (speech and the screen reader are M15, owner
2026-11-25). Each can be proposed after the release
milestone.

## 9. UI work packages

Sizing and order live in [08-milestones.md](08-milestones.md); this is the
content checklist.

* **WP-UI-1 Toolkit core**: `Gfx`, palette, fonts (UI + large numerals +
  fallback loader interface), key input mapper (phases, repeat, hold, lock),
  navigation stack, status bar, strip, `ListView`, `Menu`, `Dialog`, `Toast`,
  `VolumeOverlay`; simulator window and headless golden-image harness.
* **WP-UI-2 Home, Settings, system dialogs** (boot, power menu, lock screen,
  quick panel, errors) on the `Settings`/`Power` service traits.
* **WP-UI-3 Files** with the windowed/indexed directory source and text viewer.
* **WP-UI-4 Music** lists, Now Playing views, equaliser, queue.
* **WP-UI-5 Radio, Recorder, Stopwatch, Steps, Pictures.**
* **WP-UI-6b Remote** (§7.12): Connect, Present, Pointer, Calibrate, profiles
  and the key-map editor, simulator HID back end.
* **WP-UI-6 Bluetooth, USB dialog, USB Drive, Authenticator** (incl.
  `TextEntry` and PIN pad).
* **WP-UI-7 Polish**: icons and themes pass, animations, rotation, language
  packs, wallpaper, screen saver, full acceptance sweep on the device with
  screenshots archived in `docs/screens/`.
