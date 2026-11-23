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
| **Up / Down** | Move focus; change the focused value in editors | Auto-repeat, accelerating after 1 s (lists: 8 → 30 rows/s) |
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
  through palette roles, never literal colours.
* **Motion**: list scrolling is by whole rows (no pixel smooth-scroll). Screen
  changes: 120 ms horizontal slide of the content area only (8 steps), skipped
  when disabled, in Battery-saver mode, or when audio buffers are low. Marquee,
  progress and the spinner are the only continuous animations.
* **Rotation**: 0° and 180° (clip worn upside down), set in Display settings;
  implemented with the panel's MADCTL and a swapped key map for Up/Down and
  Prev/Next.

## 5. Toolkit (crate `yoton-ui`)

`yoton-ui` is `no_std`, has no allocator, and knows nothing about hardware. It
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
| `ListView` | Virtual: asks a `ListSource` for rows `[first, first+n)`; keeps a cache of two pages. Rows not yet loaded draw as dim placeholders. Supports sections, per-row icon/value/accessory, letter jump, wrap-around off by default |
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

### 5.3 Text and strings

All user-visible strings go through a string table (`tr!(Id)`), English built
in. Additional languages are resource files loaded into a small arena
(`.yoton/lang/xx.ylang` on the card, or in the NOR resource area); the table
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
are editable in Settings → Display → Home screen. Apps whose hardware is absent
or unsupported in the running build (see 06-connectivity §"Bluetooth") are
hidden, not greyed.

### 6.3 Lock and screen-off

Power tap, or the backlight timeout followed by the lock timeout, turns the
panel off (LCD sleep, backlight PWM off, UI redraws suspended). Playback
continues at the lowest sufficient clock. While locked, a key press shows the
lock screen for 2 s: clock in large numerals, date, track title, battery,
"Press Power to unlock". Optional screen saver instead of off: dim clock that
moves every minute.

### 6.4 Quick panel (hold Menu)

A sheet with: Now Playing (if any) · Lock · Bluetooth on/off and current device
· Output (Headphones / Bluetooth) · Sleep timer (Off, 15, 30, 45, 60, 90 min,
end of track) · Brightness slider · Shuffle · Repeat · EQ preset. Values change
with Prev/Next directly in the row, without leaving the panel.

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
* The index is a file on the card (`.yoton/library.ydb`), built by a background
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
  2.0×, pitch kept) · Equaliser… · Sleep timer… · Output (Headphones/Bluetooth)
  · Rate? (not in v1) · Go to album / artist / folder · Delete file… · Details.
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

**Acceptance**: every list reachable; 3000-entry lists scroll without stalls;
track change by end-of-track and by Next keeps the display consistent; resume
after simulated power loss; all views render with missing tags and missing art;
titles in Cyrillic, Greek, CJK render; **[HIL]** underrun counter stays 0 while
scrolling every list at full repeat rate during 320 kbit/s MP3 and 16/44.1 FLAC
playback.

### 7.2 Files

A browser over the card. Directories first, then files, natural sort
("Track 2" before "Track 10"), case-insensitive. Hidden and system entries
(`.yoton`, dot-files, `System Volume Information`) hidden unless enabled.

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
  `.yoton/cache/`, validated by a signature of the directory contents, so
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

* Prev/Next tap: step one channel (region step 50/100/200 kHz); hold: seek to
  the next station (hardware seek when the tuner provides it). Up/Down: next/
  previous preset. OK: mute. OK hold: stop the radio (powers the tuner down).
* Menu: Save preset / Remove preset · Presets… (list, rename, reorder, delete)
  · Auto-scan (fills up to 30 presets, strongest first, asks before replacing)
  · Record (opens the recorder with FM as the source) · Force mono · Region
  (Europe/US/World 87.5–108, Japan 76–95, wide 64–108; de-emphasis follows) ·
  Output (Headphones; Bluetooth if FM audio can be digitised — see 04-audio).
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

### 7.4 Bluetooth

The fitted module is an audio transmitter (06 §4): the app sends audio out and
nothing else. There is no receiver ("speaker") mode and no remote.

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
* Menu: Source (Microphone / FM radio / Line-in if the hardware has it) ·
  Format (WAV PCM · WAV ADPCM · FLAC · MP3 · Opus voice — those built, see 04)
  · Quality (sample rate, channels, bitrate) · Gain mode (manual / automatic) ·
  Voice activation (threshold, hang time) · Auto split (off, 15/30/60 min,
  2 GiB) · Pre-record 2 s · Monitor through headphones · Low-cut filter ·
  Recordings… (list in `/Recordings`, newest first, OK plays, Menu deletes/
  renames).
* Files: `/Recordings/REC_YYYYMMDD_HHMMSS.ext`, FM: `/Recordings/FM/
  FM_101.90_YYYYMMDD_HHMMSS.ext`. The header is rewritten every 10 s so a power
  loss costs at most 10 s.
* The key click is forced off while the microphone is live. Playback of other
  audio is stopped when recording starts.

**Acceptance**: state machine incl. pause, mark, split, card-full (stops and
saves cleanly at 1 MiB free) and card-removed; file validity checked with
`ffprobe` in host tests; **[HIL]** recorded FM or loopback tone has the expected
length (± 0.1 %) and no dropped blocks (encoder block counter contiguous).

### 7.6 Pictures

* Browser: 3 × 3 thumbnail grid (40 × 30 cells) with a name line in the strip,
  or list mode; thumbnails are generated lazily (JPEG DC-only decode is used
  for speed), cached under `.yoton/cache/thumbs/`, and never block navigation.
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
* Menu: Add… (import from `otpauth.txt` on the card — the file is overwritten
  and deleted after import; or manual entry: issuer, account, Base32 secret
  with the Base32 keyboard, digits 6/8, period 30/60, SHA-1/256/512) · Rename
  · Delete… · Move · Next code (HOTP) · Time check (shows RTC time, UTC offset,
  last synchronisation and estimated drift; warns if the clock was never set)
  · Change PIN · Wipe vault….
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
| Display | Brightness · Backlight time-out (5 s … always) · Screen-off lock delay · Theme · Accent · Wallpaper · Screen saver (off / clock) · Animations · Rotation 180° · Clock in status bar · Home screen (order, hide) |
| Power | Mode (Automatic / Battery saver / High performance) · Sleep timer · Auto power-off when idle (off, 5 … 60 min) · Keys when locked (none / volume / volume + transport) · Battery (percent, voltage, state, cycles if tracked) |
| Date and time | Set time · Set date · 24-hour clock · Date format · UTC offset · Correction (s/day) · Set from radio (RDS) |
| Bluetooth | Same content as the Bluetooth app's settings rows |
| Radio | Region · Force mono · Seek sensitivity |
| Recording | Default source · Format · Quality · Gain mode · Folder |
| USB | When connected (Ask / Charge only / File transfer / Keyboard / last image) · Read-only card reader · Developer channel (on in development builds) |
| Language | English · (packs found on the card) |
| Storage | Card info · Rebuild library · Clear caches · Check card · Format card… |
| System | About (version, build id, serial, hardware, licences) · Update firmware… · Diagnostics · Start original firmware (dual-boot layout) · Factory reset… · Recovery mode |

Diagnostics (also the bring-up aid for workers): key test (shows raw ladder
values and decoded keys), display test (colour bars, gradients, pixel grid),
audio test (1 kHz tone left/right, sweep), sensor live view, card speed test
(sequential read/write MB/s, random read latency), memory and pager statistics
(free pool, stack high-water mark, faults/s, resident size), clock and power
state, event list (the last 32 coded events: boot reasons, card errors,
underruns, faults survived; 02-architecture §8).

**Acceptance**: every setting round-trips through the store; factory reset
restores defaults and leaves the card untouched; golden images for each group.

## 8. Non-goals for v1 (recorded so nobody builds them by accident)

Video playback; games/plugins; touch or pointer UI; a remote-control app or
mouse for a computer; a Bluetooth receiver mode; multi-language text shaping
(Indic, Arabic joining); streaming/network features; user theming beyond
palette + wallpaper; voice prompts. Each can be proposed after the release
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
* **WP-UI-6 Bluetooth, USB dialog, USB Drive, Authenticator** (incl.
  `TextEntry` and PIN pad).
* **WP-UI-7 Polish**: icons and themes pass, animations, rotation, language
  packs, wallpaper, screen saver, full acceptance sweep on the device with
  screenshots archived in `docs/screens/`.
