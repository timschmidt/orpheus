# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | `docs/PLAN.md`, `docs/plan/`. **D-2 decided 2026-11-24 by the user: full-flash layout** ("wipe the original firmware and take over the full player hardware") |
| Code | Host workspace (`crates/*`, `sim`, `xtask`) and device workspace (`fw/`: `atj2157-hal`, `yoton-bsp`, `firmware`, `boot/stage2`, `payloads/norprog`, `bringup/*`) |
| Player | **Runs Orpheus from NOR, full layout.** Stock firmware and the Rockbox port are erased. Card: the owner's music (exFAT/FAT, 238.3 GB), read only except `/.orpheus/` |
| NOR | Sector 0: Rockbox-port stage 1 (unchanged). `0x001000`: full slot A (Orpheus, 544,768 B). `0x7DA000` settings, `0x7DE000` vault, `0x7DF400` Rust stage 2 (876 B, tries full A, full B, dual slot; no stock-firmware path). `0x7E0000–0x7FFFFF` vendor data, untouched. Backups: `yoton/dumps/nor_backup_8m.bin` (stock, verified), `local/nor-before-full.bin` (the chip just before the switch), `local/rockbox-nor-0x762000.bin`, `local/stage2-previous.bin`. Way back: `xtask dev restore vendor --i-have-a-backup` (`--verify` compares) |
| Host | Fedora 43, Rust 1.98.1 pinned. Root disk nearly full (≈ 14 GB free after deleting worker build output) |
| Current milestone | M1–M3 device parts done; M4 (Files/Music) and M5 (playback) running on the player; host side of M6–M8 packages largely done |

## 2. Work packages

| WP | Status | Notes |
|---|---|---|
| T-1, T-2, T-4a, HAL-0, HAL-1, T-5 | done | see earlier log entries |
| T-3, PG-1 | done | pagelink, PagerCore, check-resident; demand paging on the player (8 KiB pages, 1 KiB lines) |
| T-6 | started | Rust NOR payload `fw/payloads/norprog` (erase/program, full-layout guard); ymon and nordump still the reference repo's C payloads |
| LAY-1 | done (device) | Full layout installed (`xtask dev install-full --i-have-a-backup`), layout-aware `stage`, `restore vendor [--verify]`. Unifont pack in NOR not done (the UI's fonts are built in) |
| LCD-1, INPUT-1, PWR-2, AU-0 | done pending user checks 7, 8, 9, 10 | |
| SD-1, FM-0, USB-1, USB-3, PWR-1 | done | |
| SYS-1 | done (first cut) | Firmware: tasks, settings in NOR, developer channel, watchdog task fed only on RT + main + UI progress (verified: a hung main executor resets in ≈ 3 s), test safety window |
| PWR-2 governor | first cut | 48 ↔ 96 ↔ 144 MHz from main-executor idle share |
| PWR-3a | mostly done (device) | Power-off: card flushed, Bluetooth 0x24, screen off, vendor PMU sequence (wake on long press or USB plug-in); in test mode a reboot instead (verified). Auto-off (setting, no key, nothing playing, no cable) and low-battery off (< 3.30 V for 10 s, no cable) through the same path — not testable with the cable in: user checks 11 and 15. Alarm wake not done |
| RTC-1 | done (device) | set/read over the developer channel (`xtask dev time [--set]`); alarm untested; once lost after a ymon reset (U-RTC open) |
| USB-4 | done (device) | Card-reader personality: card handed between the card task and USB; write-protected test mode; mid-track switch and hand-back verified. 2.3 MB/s read (open) |
| FS-1 | done | `orpheus-fs` |
| FS-2 / AU-5 firmware | first cut on the player; **`orpheus-player` wired in; gapless hand-over and queue edits (built, not yet run on the player)** | Files lists the card (sorted through the directory index since AU-12 firmware), text viewer window, Now Playing. The card task drives `orpheus_player::Player`: folder queue in RAM and `/.orpheus/queue.bin`, resume record and bookmarks in `/.orpheus/player.bin`, seek, shuffle/repeat (settings), A–B, sleep timer, Up next. Gapless read-ahead (`orpheus_audio::StreamRing`) and decoder re-open with the DAC running; Up next remove/move, Play next, Add to queue (256 max). Not done: fades. User checks 13, 17 |
| UI-1, UI-2, UI-3, UI-4 | done (host); running on the player | 457 golden images |
| AU-1…AU-4, AU-7, AU-8, AU-9 | done (host); on the player: MP3/FLAC/WAV and Speex at 48 MHz, Vorbis at 144 MHz (clock floor), **Opus too slow** | Opus CELT touches more code per frame than the 32 KB code cache holds; ends with "decoder too slow for this player" (abort after 8 underruns) instead of a reset. Needs a bigger code cache or a resident hot path (RAM decision). Ogg pages over ~15.5 KB fail (16 KiB ring < `MAX_PEEK` 28 KiB; RAM) |
| AU-6 | done (host); **working on the player** | `orpheus-dsp` chain between decoder and DAC, fed by the Sound settings; works on a scratch copy of the decoder output; while any stage is active during playback the clock floor is 96 MHz (EQ + bass + crossfeed: 0 underruns, ≈ 90 % idle). WSOLA not done; Levels meter not wired |
| AU-12 (directory index, music library) | host done; **firmware wired in (built, not yet run on the player)** | `orpheus-index` + its `card` adapters over `orpheus-fs` (host-tested on FAT32/exFAT images). Files/Music › Folders sorted (Name/Date/Size, hidden per setting), letter jump from the index's letter runs, play queue in listing order. **Music library** (`orpheus-library`) built in the card task's idle loop, Music › Artists/Albums/Songs/Genres/Recently added, Play/Shuffle, Shuffle all from the library, Go to album/artist. Playlists, Favourites: not started. User check 16 |
| AU-14 | done (host); **device: Pictures app and album art (built, not yet run on the player)** | `orpheus-img`. Pictures (folders + pictures, list mode), viewer (fit/fill, rotate, next/prev, slideshow), album art made at track open and cached in `/.orpheus/cache/art`. Not done: thumbnail grid, 100 % + pan, wallpaper, delete, per-file rotation memory. User check 14 |
| OTP-1, BT-1, USB-6 | done (host) | `orpheus-otp`, `orpheus-bt`, MMC/CD-ROM in `orpheus-usb` |
| BT-2 (device) | **built, not yet run on the player** | UART1 driver (`atj2157_hal::uart`, `yoton_bsp::bt`), `orpheus_bt::driver`, firmware `bt` task, `BluetoothService`, shutdown 0x24 on power-off. Module off at boot; `bt probe/on/off/status/scan/connect/listen/shutdown/rx/tx` test commands. Not done: volume policy, metadata at track change, pairing UI, U-BT-2 |
| UI-6a | **on the player 2026-11-24** (connect, reconnect, audio, volume — policy changed, see session entry) | Bluetooth app (status, Pair new device, Paired devices, connect progress/failure, forget, reconnect switch), `BluetoothService` lists/requests, volume policy of 06 §4.2 (0x26 on connect and change, DAC reference −3 dB while the sink takes absolute volume), address text in written order. User check 17 |

## 3. Session log

### 2026-11-24 — UI-6a: the Bluetooth app (worker, no hardware)

* **Screens** (`orpheus-apps::screens::bluetooth`, ids 27–30): Home →
  Bluetooth (enabled in the firmware's `app_available`): header, Bluetooth
  on/off, state row (Off / Starting… / Not connected / Searching… /
  Connecting… / Connected / Sending audio / Not responding, with the status
  icon), the connected device on its own row (OK = Disconnect? / cancel),
  Pair new device… (starts the inquiry), Paired devices (count), Reconnect
  automatically (0x0603). **Pair new device** (`BtScan`): live list, kind
  icon from the class of device (headphones / speaker / phone / computer /
  generic; major class is in the middle byte whichever the order), name or
  the address, names arriving after the inquiry; "Searching…" while it
  runs, then "No devices found" / "Search again"; OK connects (the inquiry
  is stopped first), Back stops it. **Paired devices** (`BtPaired`): radio
  marks the connected one; OK connects or asks to disconnect; Menu →
  Connect/Disconnect, Forget… (destructive dialog). **Connecting**
  (`BtConnect`): spinner, Back cancels (0x29); success pops back, a global
  toast "Connected · NAME" (also for reconnects; "Disconnected" when a
  connection is lost); failure (module 15 s timeout, refusal, our 40 s
  guard) → dialog "Could not connect" with Cancel / Retry. No rename (no
  such module command, bluetooth.md §3.7), no signal strength (byte 6 of
  0x03 unknown).
* **Service API** (`services.rs`): `BtPhase`, `BtAddr` (`push_text`
  reversed), `BtKind::from_cod`, `BtDevice` (by value, 24-byte name, cut
  on a character), `BtList`, `BtRequest {Scan, StopScan, Connect(addr),
  CancelConnect, Disconnect, Forget(addr)}`, `bt_phase/bt_count/bt_entry/
  bt_request`, events `ServiceEvent::Other { kind: other::BLUETOOTH, value:
  bt_event::{LISTS, PHASE, CONNECT_FAILED, CONNECTED, DISCONNECTED} }`.
  `settings::id::BT_RECONNECT`.
* **`orpheus-bt`**: connect during an inquiry stops it (0x01) and connects
  at its end (cancel before that drops it); names of unnamed devices are
  asked for (0x02) when an inquiry ends; a newly connected device joins the
  paired list with its found name; **the player's volume** (`set_volume`,
  now remembered when not connected) **is sent as 0x26 on every
  connection** before the audio start, and on every change; a sink report
  that differs within 3 s of connecting (BlueZ starts at 0) or our query's
  answer is answered with ours, later sink changes are taken
  (`Output::SinkVolume`), `Output::VolumeAcked` on the acknowledge of
  0x26. `Addr::to_text` prints the bytes reversed (the lead's finding:
  LSB first on the wire). `local_from_ui`/`ui_from_local` (UI 0–30 ↔
  module 0–31, round trip exact). Tests: 34 (new: connect during inquiry,
  cancel before the inquiry stops, connect → 0x26, UI volume round trip).
* **Firmware**: `bt` task requests `StopScan, ConnectTo, CancelConnect,
  Forget`; `LISTS` (found + paired, ≈ 0.5 KB, copied in place when they
  change); `Snap` + phase, counters of connects/fails/drops/list changes,
  sink state; `bt::VOLUME` (atomic) carries the player's volume to the
  task. `Fw`: volume policy (06 §4.2, the lead's measurement: the tap is
  after the DAC volume): DAC held at UI step 30 (−3 dB) while connected to
  a sink that acknowledged 0x26 or reported a volume, back to the
  player's volume on disconnect or when the sink says it has no absolute
  volume; sink changes become the player's volume. `atj2157-hal/dac.rs`
  untouched (the lead's).
* **Simulator**: fake module (`sim/src/bt.rs`: start-up 1.2 s, five
  devices appearing over 2.7 s, one unnamed until after the inquiry, one
  never named, Car Kit and the phone fail to connect, forget, reconnect at
  power-on), `set:bt=off|starting|on|scanning|connecting|connected|
  streaming|dead`, `set:btfound=N`. 91 goldens `bt_*` (15 journeys × dark/
  light × Latin/Cyrillic/CJK + amber), looked at; Russian/Japanese strings
  shortened until nothing important is cut; 7 kanji added to the Unifont
  subset (from an older Unifont, CJK ideographs unchanged). Behaviour tests
  `sim/tests/bluetooth.rs` (9). All older goldens unchanged.
* **RAM** (`xtask build`, this worktree): `__stack_bottom` `0x131c00` →
  `0x131f38` (**+824 B**): bss +608 (`LISTS` 532, `SNAP` and the UI's copy
  +2 × 16, task/`Fw` fields), resident +216 (rodata, panic-location data;
  expect less in the main checkout). `xtask size --check` fails in a
  worktree as before (resident 83,468; base 83,252).
* **Facts from the lead's run** (01-hardware §9 U-BT-2, U-BT-6):
  addresses LSB first; sink volume 0 after connect; connect refused during
  an inquiry; SDP record advertises HID 0x1124; reconnect within ~10 s;
  the feed tracks the DAC volume. `docs/hw/bluetooth.md` is not in a
  worktree (ignored): the note for its §8 Q8 is in the UI-6a report.
* Not done: track metadata (0x31) at track change; "Output" setting and
  muting the wired output while streaming (U-BT-2 open); details page
  (address, profiles); refresh of the paired list read from the module
  after pairing (the local insert stands in); the reference level is not
  measured for clipping (user check 17).
### 2026-11-24 — AU-5 firmware: gapless track changes and queue edits (worker, no hardware)

* **Gapless read-ahead** (04 §2.2). New `orpheus_audio::StreamRing`
  (host-tested): one byte ring holding the current stream and, once the
  reader has read that file to its end, the head of the next one right
  behind it. The decoder sees only the current stream (`ByteSource`/
  `SeekSource`, file offsets); `hand_over` makes the next stream current
  (its offset 0 directly after the old stream's last byte; unread trailing
  tags dropped). A seek in the current stream drops what was read of the
  next one (read again from its start). No panic paths (each new file path
  would land in resident `.rodata`). The firmware's `Ring` wraps it and now
  owns the files too (`Ring::file`, `next_file`: a `File` is 64 B and is no
  longer moved through the RT task's frames).
* **Flow.** `ReaderAtEnd` → player `Prefetch(item)` → the card task opens
  the item (directory lookup, title kept, not shown) between blocks and the
  ring reads it after the current file. `DecoderEnded` → `Open { gapless }`:
  if that item is the one read ahead, the DAC is **not** drained or stopped
  (`PcmOut` now lives in `run_player`, across tracks); `play_item` hands the
  ring over, publishes the title, and `play_file` probes (ID3 skip through
  `skip_to`), inspects and opens the next decoder in the same arena on the
  bytes already there; the DSP chain keeps its state when the format is the
  same (no `apply_sound`, `set_format` is a no-op then). The decoders trim
  encoder delay/padding themselves (LAME/Xing, Ogg granules, FLAC totals),
  so nothing is inserted or dropped at the join. The clock floor is High
  from the decoder's end until the next track's codec sets its own (Vorbis
  codebook set-up while the PCM ring plays out); underruns in that window
  do not count towards the too-slow abort. Album art (cache only:
  `pictures::track_art_cached`, else the folder's cover when it is the same
  folder) and the library place (`find_now`) follow at the next ring
  refill. No save at a gapless change (the DAC runs; the next pause, stop,
  ordinary track change, hand-over or power-off saves).
* **Fallbacks.** Another sample rate: `play_file` returns `End::Again`,
  `play_item` lets the previous track play out (`PcmOut::finish`) and opens
  the same item the ordinary way (DAC restarted at the new rate). A file
  read ahead that cannot be opened: the same (it then fails as usual). A
  track change the user makes (Next, Prev, jump, new queue, bookmark) is
  unchanged: stop, flush, fresh start. `CancelPrefetch` (repeat/shuffle
  change, queue edit, sleep after track) drops the read-ahead and reports
  `ReaderAtEnd` again so the new next track is read ahead.
* **Host test of the hand-over** (`orpheus-codec-conformance`):
  `join::decode_gapless` is the firmware's sequence (16 KiB `StreamRing`,
  512-B-minimum reads, prefetch when the reader is at the end, probe → tag
  skip → inspect → open in one arena). `tests/ring_join.rs`: a WAV signal
  cut at 1 / 4,093 / 33,333 / 99,999 frames with a `LIST` tag after the
  first part and a 19 KB ID3 tag (larger than the ring) before the second
  joins sample-exact; FLAC×3, MP3×4, 24-bit FLAC×2 and a mixed
  MP3/FLAC/MP3/FLAC chain equal the parts decoded alone. `join::check` (the
  corpus album splits of `xtask test codecs`) now also requires the ring
  path to give exactly the per-part samples: `xtask test codecs --filter
  gapless --quick` (fresh corpus) passes flac/wav bit-exact, mp3 (LAME)
  −118.7 dBFS and vorbis −121.2 dBFS vs the per-track references, all
  220,500 frames = source length. Shifting the hand-over by two bytes
  makes the tests fail (checked).
* **Queue edits** (05 §7.1). `PlayerCmd::PlayNext(file)` (after the current
  track), `AddToQueue(file)` (at the end), `Remove(i)`, `MoveUp(i)`,
  `MoveDown(i)` (Up next; only tracks after the current one) →
  `PlayReq::Edit` → `Server::edit` → `orpheus_player::queue::edit`
  (host-tested): a shuffled queue is first written into the queue in its
  play order (`unshuffle`, in place, 256-bit bitmap), so the order the
  user sees stays and the player goes on unshuffled; the new
  `Command::Requeue { id, len, pos }` updates the player without touching
  the track (prefetch cancelled and asked for again; resume record saved
  with the new queue id unless stopped). Queue id and Up-next generation
  change; `queue.bin` is rewritten at the next save (`Server::queue_dirty`,
  before the resume record in `save_store`). Limit 256
  (`services::QUEUE_MAX`): Files shows "The queue is full" (new string, ru/ja
  in the sim packs) instead of sending. With nothing queued, Play next / Add
  to queue start a one-track queue. The Shuffle setting stays on after an
  edit; switching it off keeps the edited order, switching it on shuffles
  anew.
* `orpheus-player`: `Command::Requeue`, `Player::order`, `queue::{edit,
  unshuffle, Edit, EDIT_MAX}`; 22 tests (+5: requeue plays on gapless in the
  new order, requeue of a shuffled queue keeps the order and the record,
  edits, unshuffle, shuffled edits). The test simulation re-reports the
  reader's end after `CancelPrefetch`, as the firmware does.
* **RAM / stack** (`xtask build`, this worktree, before → after):
  `__stack_bottom` `0x131c00` → `0x131d38` (main stack −312 B: `Ring` +2
  files + read-ahead record ≈ 250 B in the audio task's future, `Server`
  +4); resident 83,252 → 83,252 B (unchanged; `xtask size --check` fails in
  a worktree as before: 83,252 > 81,920, the absolute panic-location paths).
  Frames (`sub sp`): audio task poll 5,024 → **4,992 B**, `play_file`
  2,380 → **2,332 B**, `save_store` 916 → 908 B; new functions all below
  256 B (`Ring::hand_over`, `Server::edit`, `prefetch`, `track_meta`).
* Not done: fades (04 §2.2) around pause/seek/manual skips; saving the
  resume point at a gapless change; making art for a track whose embedded
  picture is not cached yet at a gapless change (the folder's cover or none
  is shown until the next ordinary open); Play next / Add to queue for
  library rows and whole folders (the menus offer them only on Files
  entries); a golden for the "queue is full" toast. Not run on the player:
  user check 18 and the lead's test below.
* Device test plan (lead): `cargo xtask test codecs --gen` makes
  `tests/corpus/gapless/{flac,wav,mp3,vorbis}_part{0,1,2}.*` (a 5-s music
  source cut at frames 60,417 and 131,333). Upload each codec's three parts
  into its own folder, e.g. `cargo xtask dev put
  tests/corpus/gapless/mp3_part0.mp3 /.orpheus/test/gapless-mp3/mp3_part0.mp3`
  (and so on), plus one 48 kHz file into `/.orpheus/test/gapless-mp3/`
  named `mp3_part3_48k.mp3` for the rate fallback. On the player: Files →
  Menu → Show hidden → `.orpheus/test/gapless-mp3` → OK on `mp3_part0`;
  during part 1 and 2 `cargo xtask dev cmd status` must show
  `underruns=0` (the counter restarts per track and covers the hand-over),
  the title changes at each join, `cargo xtask dev stats` shows the clock
  at High briefly and back to the codec's floor. Listening: user check 18.
  Then queue edits: Up next → Menu → Move down / Remove on the next track
  while the current one is in its last seconds (the edited next track must
  follow gapless), Files → Menu → Play next / Add to queue, and 256+ adds
  for the toast; after each edit `cargo xtask dev cmd status` and a
  power-off/on: Music → Resume continues in the edited queue.

### 2026-11-24 — AU-12 firmware: the music library on the player (worker, no hardware)

* **Flow.** At mount the card task opens the newest complete library
  (`/.orpheus/library/lib-a|b.ydb`). Without one a build is *due* 10 s
  later; after a card-reader session an update is due 5 s after the card
  comes back (unchanged folders are copied without a tag read); Settings ›
  Storage › Rebuild library (`Req::Rebuild`, from the UI's storage action)
  makes it due at once. A due build runs only in the idle loop (no track
  loaded, after a waiting directory-index build) in steps of 32 units (a
  tag read counts 8) on the last 20 KiB of the decoder arena, sleeping as
  long as each step took (≥ 2 ms). Any request, command or player request
  pauses it (`Suspended::pause_in`: buffers flushed, scratch free); the
  idle loop resumes it (a timer wakes the loop when a build becomes due).
  A card hand-over gives a build up (the next build empties its files).
  The UI sees `LibraryState::Building { files }` while there is no library
  yet, `Ready` once one is open (an update keeps the old one usable until
  it is done, then the lists are asked for again).
* **Build state in place.** `orpheus-library` gained `Suspended::new`,
  `step_in`, `pause_in` (the builder's methods now run on a `Run` that
  borrows the state; `Builder` is unchanged outside): the firmware keeps
  the 1,096-B state in the **last bytes of the decoder arena** (no
  playback uses them: Opus 59,488 B + 2 KiB spare leave 4 KiB; the arena
  everything else is lent is shorter by that much; a const assert guards
  it) and drives it where it is — no `Builder` moved through the stack
  (the first version had 2.3 KB / 1.2 KB transient frames for
  suspend/resume and a 1.1 KB static).
* **Card adapters.** `orpheus_index::card::CardIo::new_in` (Storage files
  flat in another folder); `orpheus_library::card::{CardTags, TagState}`
  (TagSource over `orpheus-fs`: `open_dir` with `after` = seek to the
  entry and read past it; `open_file` opens the entry just listed without
  a directory seek). `LetterTable::begin/push` (letter runs from any
  source). Host test (`orpheus-library/tests/card.rs`): FAT32 and exFAT
  images (mkfs), 90 tracks of the mock card written as files, first build
  in 1–7-step sessions with suspends, names identical to a mock-card
  build, `find`, album queue, an update driven in place reads 0 tags,
  only the slot file left, `fsck` clean.
* **Screens → queries.** New `LibraryService` (in `Services`) and
  `LibraryScreen` (screen 26, one per level, resume record = list code +
  focused row id): Music › Artists → `List::Artists` → OK → `ArtistAlbums`
  → `AlbumTracks`; Albums → `Albums`; Songs → `Songs`; Genres → `Genres` →
  `GenreArtists` → `GenreArtistAlbums` → `GenreAlbumTracks`; Recently
  added → `Recent` (100 rows). OK on a track: `PlayerCmd::PlayLib { list,
  start: row }` → `Library::queue` of up to 256 tracks around the row;
  Menu → Play / Shuffle (focused container's tracks, or the track list),
  Play all / Shuffle all (an artist's or genre's tracks). Shuffle all:
  `Library::sample` (256, library order, played shuffled); the card walk
  stays as the fallback without a library. Now Playing menu: Go to album /
  Go to artist (when the track is in the library: `Library::find` at track
  open → `PlayerSnap::lib_now`), landing on the track / album. Unknown
  artist/album/genre strings (en/ru/ja). Library lists share the browse
  window (pseudo directory `0xF…`, rows carry kind, row id, duration;
  `tag::LIBRARY`), letter runs go into `media::LETTERS`.
* **Simulator**: `set:library=ready` builds a real library from the card
  image through the same adapters (sessions of 64 steps); 26 new goldens
  `library_*` (root, artists, an artist's albums, album tracks, albums,
  songs, genres, recent, menu, play, Now Playing menu, Go to album / artist;
  dark/Latin and light/CJK), looked at; all 479 older goldens unchanged.
* **RAM** (`xtask build`, this worktree): `__stack_bottom` `0x1320c8` →
  `0x132600`, main stack 21,304 → 19,968 B (**−1,336 B**): resident
  +1,248 (rodata +976 — the panic-location paths of 9 newly linked files,
  `orpheus-library` ×5, `orpheus-meta` window/length, `orpheus-index`
  extsort…, ≈ 42 B shorter each in the main checkout, so ≈ −380 B there;
  data +280: `SERVER` +256 = `LibState` with the open `Library`, `PLAYER`
  +24), bss +88 (UI future +80). Expect ≈ −960 B in the main checkout.
  `xtask size --check` fails **in a worktree** (resident 83,244 > 81,920;
  the base was already 81,996 here); 103 paths × 42 B ⇒ ≈ 78.9 KB in the
  main checkout.
* **Stack frames** (`sub sp`/`subw`): audio task poll 5,088 → 5,024 B,
  UI task poll 6,016 → 5,984 B. New: `library::step` 2,036 (the builder's
  scan with `orpheus_meta::Tags` inlined), `Run::sort` 1,100,
  `Library::open` 1,340 (mount/hand-back only), `CardTags::open_dir`
  1,044, `publish_list` 860, `fill_queue` 628, `begin` 620,
  `fill_sample` 580, `load_letters` 508, `session` ≈ 320, `pause` 116.
  Deepest new RT path ≈ audio poll + step + `Out::put` → `CardStore::
  write_at` 640 → `Inner::slot` 2,500 → FS open ≈ 11.6 KB, below the
  playing-and-serving path (≈ 13.6 KB) — check the stack high-water mark
  during a first build.
* Not done: an update at every mount (a card changed in a computer while
  the player was off is only picked up by Rebuild or a card-reader
  session); Playlists; "Rebuild" is always an update (never from
  scratch); Ogg/MP4 tags (names from files/folders); no build progress
  in percent (the row says "Building library…"); a build that fails is
  not retried until the next mount, card-reader session or Rebuild.

### 2026-11-24 — BT-2 device side: UART driver, Bluetooth task, shutdown on power-off (worker, no hardware)

Continued an interrupted worker's WIP (merged, reviewed, mostly kept).

* HAL `uart` (new): open/close (reset pulse, clock gate, `BR`, `CTL`),
  error flags, CPU FIFO read/write, TX by DMA (vendor channel-9 word
  `0x00228200`, `TFER` cleared before/after, wait for `UTBB` — VERBATIM
  bluetooth.md §2.3; bounded, aborts on timeout, counts a missing
  completion interrupt), RX `DmaRing`: single-byte DMA with reload into a
  ring, write position from the remain counter (U-BT-4). No UART
  interrupt (an RX-interrupt ring was written and dropped again for its
  resident RAM; it is the next fallback).
* BSP `bt`: pins (GPIO38 TX / GPIO39 RX function 10, pads `0x302A` /
  `0x312A` VERBATIM, GPIO20 enable, GPIO31 host-busy low), `up`/`down`
  (enable line last up / first down, pads parked low when off as the
  vendor does), `Listen` (both UART pads as pulled-up inputs, nothing
  driven, edges counted while the module starts: board side of U-BT-1).
* `orpheus-bt`: `driver` (sans-I/O glue: `Port` trait, power on/off,
  bounded power-off shutdown, stats, the enable line re-checked against
  the state after every drain); transmitter `shutdown` (0x24 once, no
  resend, refused unless the module is up; no restart afterwards); the
  command queue is now a 264-byte ring of encoded frames (was 8 × 81 B);
  TX frames are sent straight from the command layer's buffer;
  `MAX_FOUND` 8, `MAX_NAME` 24, 12 outputs (RAM). The crate has no panic
  sites left in the firmware (their absolute-path location strings sit in
  resident `.rodata.str1.1`: ~0.6 KB in a worktree, ~0.3 KB in the main
  checkout). Tests: 30 (shutdown with/without acknowledge, power-off while
  starting, dead module, start-up/scan/connect over a mock UART, a noisy
  byte-by-byte line, the queue ring, lost enable output).
* Firmware `bt` task (main executor): module off at boot (GPIO20 driven
  low); the Bluetooth setting powers it 5 s after boot, never in a host
  test; setting changes switch it; headset keys → player; playback
  start/stop → command 0x05; state and device name → `BluetoothService`;
  a newly connected device is saved (settings key `0x06F0`) as the
  reconnect target. `Ui::power_off` sends 0x24 when the module is up and
  waits at most 1 s for its completion (owner decision 2026-11-24); a test
  run reboots before that point, so `bt shutdown` exercises the same
  sequence by hand. No other restricted command is sent anywhere.
* Developer channel (`xtask dev cmd "bt …" --wait N`): `probe` (on,
  start-up exchange, full report, off again; ≤ 25 s), `on`, `off`,
  `status`, `scan`, `connect N` / `connect pN`, `disconnect`, `listen`
  (≈ 4 s), `shutdown`, `rx dma|cpu`, `tx dma|cpu` (module off only). The
  report: state, identity, pad words of GPIO20/31/38/39, UART `STA`/`CTL`,
  ring remain counter and peak, RX bytes/frames/skipped/bad length/bad
  checksum, TX frames/bytes/failures/DMA timeouts/missing completion
  interrupts, command counters, paired and found devices, the last 24
  bytes received.
* Diagnostics text uses `t!` statics so it is paged (plain byte-string
  literals go to the resident mergeable string section).
* RAM (`xtask build`, worktree): `__stack_bottom` `0x131728` → `0x132018`,
  stack −2,288 B (bss +2,136: driver 1,328, task 336, ring 256, request
  channel 60, snapshot 40, UI task +88; resident +160). Over the ~2 KiB
  aim by ~240 B; levers: ring 128 B (−128), fewer found devices.
* Not run on the player. Resident additions: none (no new handler; the
  DMA completion handler is `atj2157_hal::dma::*`). `check-resident` ok.
* Open: U-BT-1 (board side), U-BT-4, U-BT-5 (01-hardware §9); volume
  policy, metadata at track change, the pairing screens and U-BT-2 are the
  rest of BT-2.
### 2026-11-24 — AU-14 device: Pictures and album art (worker, no hardware)

* Started from an interrupted worker's unbuilt WIP (merged
  `worktree-agent-aeef1db8813ec5d0a`). Kept: `PictureService`
  (`show_picture`/`picture_state`, `PicReq`/`PicState`, `data::PICTURE`)
  replacing `PlayerService::art`, `PlayerState::art` (a generation), the
  Files screen's `BrowseMode` (Files / Music › Folders / Pictures with
  `BrowseFilter::images_only`), the viewer screen, the strings,
  `thumb::ART` at 52 × 52 and `ART_BIG` (128 × 128), `lookup_into`,
  `Gfx::to_screen`, `Shell::overlay_shown`. Changed: the viewer's search
  (no scan budget that could stall), a dropped request (`Pending` after
  `Data(PICTURE)`) redraws and asks again, `Busy` re-checks every tick
  and retries when the music stops, fixed light/white text on black in
  every theme (was unreadable in Light); Now Playing marks the art when
  it arrives even if the clock changed meanwhile (the WIP lost it), Big
  art no longer repaints every second (only the 2-px progress line; the
  overlay is drawn over the art in the frame buffer, the art is asked for
  again only when the overlay goes), battery/minute/volume/USB events no
  longer force a full redraw. Dropped: `scale::rgb888`/`feed_rgb565`
  (unused).
* `orpheus-index`: `Spec::images_only` (filter bit 4; index files of the
  Pictures listing are their own `…-n4.idx`; older files still parse).
* `orpheus-img` (host-tested): `art::cover_rank` (`cover`, `folder`, `front`,
  `album`, `AlbumArt…` with a picture extension), `art::Tee` (one decode,
  two scalers), `CacheKind::meta` (an entry's metadata without reading
  its pixels), `Orient::quarter_turns`.
* Firmware, new module `pictures` (card task). **Pixels go straight into
  the UI's frame buffer**, never into a RAM copy: a screen asks while it
  draws; after that frame is on the panel the UI lends the frame buffer
  (`Req::Picture(PicJob)` with its address and the box) and draws nothing
  until `tag::PICTURE` comes back; the screen then marks the box. The UI
  takes a job back that has not started (any key but volume, or 3 s:
  e.g. the card is with USB) through `PIC_SLOT` (compare-and-swap on the
  job number, so a taken-back or replaced job never starts) and cancels a
  running decode with `PIC_CANCEL`. A request made under a menu, dialog,
  toast or the volume overlay is not sent; the screen is redrawn (and
  asks again) once the overlay is gone. A lost data-ready tag is noticed
  within a loop (`PIC_SLOT` back at 0).
* **Decoding schedule.** Picture files decode only while no track is
  loaded, in the idle loop, with the whole 64 KiB arena and the stream
  ring as the source, clock floor High, sleeping 1.5 ms after every 8 ms
  of work (thread mode and the RT beat keep running). While a track is
  loaded (playing *or paused*) only ~2 KiB of the arena are free: the
  viewer shows "Stop playback to view pictures" (chosen over small-only
  decoding: even BMP/QOI need the scaler's 3 KiB) and retries by itself
  when playback stops. **Album art** is made when a track opens, before the
  codec takes the arena: cache lookup (`/.orpheus/cache/art`, 52 × 52
  `Axxx.YTC` and 128 × 128 `Bxxx.YTC` buckets) keyed by the track's
  locator, size and mtime → else the embedded front cover (ID3 `APIC`,
  FLAC `PICTURE` via `orpheus-meta`; tag reader and tags placed in the
  arena) → else the folder's best `cover/folder/front…` picture (cached
  under the cover file's key; looked for once per folder). One decode
  feeds both sizes (`Tee`); the big copy only for JPEG/BMP/QOI (PNG and
  GIF need the arena), retried small-only if the arena is short. Misses
  are recorded as a 0 × 0 entry so the next open does not look again.
  Showing art is a cache read into the frame buffer (5.4 KB small, 32 KB
  big), served between ring refills while playing; Big art without a big
  copy shows the small one centred on black. The art is never resident.
* Pictures on Home (`app_available`); Files opens pictures in the viewer
  too (next/prev skip non-pictures).
* Simulator: `PictureService` decodes for real (`orpheus-img`, 64 KiB
  arena) from a second read-only mount of the card; the card has
  `Aurora.png`, `Lake.bmp`, `Sunset.qoi` (generated, `sim/src/pictures.rs`,
  part of the card image's cache key) besides the two damaged stubs.
  22 new golden images (`pictures_*`: list, menus, PNG, BMP, QOI
  letterboxed, rotated, filled, damaged, busy; dark/Latin and light/CJK),
  `media_files_notice_*` now opens `/Images` (pictures open the viewer).
* RAM (`xtask build`, `__stack_bottom`, this worktree): main stack
  23,768 → 21,696 B (−2,072). Resident rodata +1,760 B: the panic-location
  paths of 14 `orpheus-img`/`orpheus-meta` files (≈ 1.35 KB here; each path is
  42 B shorter in the main checkout, so expect ≈ −1.5 KB there) plus the
  new strings in the merged string section; `.data` +144 (`SERVER`: art
  key, folder art, generation; `PIC_RESULT`), `.bss` +160 (UI future +128,
  picture statics). Audio task future unchanged (6,192 B). Paged: code
  52 → 62 pages (`orpheus_img` 45 KB, `Decoder::step` alone 28.5 KB;
  `orpheus_meta` 11.5 KB), rodata 14 → 16; slot 626,688 → 724,992 B.
  Deepest new RT paths: art from the cache while playing nests ≈ 2.2 KB +
  file system under `play_file` (the existing browse path ≈ 1 KB);
  `Decoder::step` 776 B + IDCT 352 B. Check the stack high-water mark
  after Big art during playback.
* Not done: thumbnail grid and its cache, 100 % with panning, rotation
  remembered per file, wallpaper, delete, Details screen (a toast),
  animated GIF, unsynchronised ID3 pictures, Vorbis/Opus embedded art,
  art in the mini player.

### 2026-11-24 — AU-12 firmware: the directory index in the card task (worker, no hardware)

* `orpheus-index` additions (host-tested): module `card` — `Storage`,
  `DirSource`, `NameSource` over a `orpheus-fs` volume. The traits are
  synchronous, `orpheus-fs` is async: each call is polled to completion
  with the caller's task `Context` (`card::spin`; the SD driver's timers
  register with the card task's waker). Index files live in
  `/.orpheus/index/X/DDDDDDDD-SF.idx` (X = last hex digit of the directory:
  16 shards keep name lookups short with thousands of folders), merge
  scratch in `/.orpheus/index/sort-a|b.tmp` (or `sort-c|d.tmp` for a build
  that runs while another is suspended); directories created on first
  use; open files cached per session (`Handles`) and closed at its end.
  `Index::read_letter_runs`, `LetterTable<N>` (letter runs in RAM, 4 B
  each, answers `letter`/`seek_letter` exactly like the index). Tests:
  build in slots that suspend/resume and close their files, a second
  build on the other scratch pair in between, query, find, rebuild after a
  change, `fsck` clean, mounted with the `/.orpheus` write prefix (FAT32
  and exFAT images made with `mkfs`); letter table vs index on random
  folders.
* Firmware flow (`media.rs`): every `Browse` makes the usual counting pass
  and folds `Signature::add` into it. Then `Index::open(dir, spec)`; if it
  is current the listing is served from it at once (`sorted: true`, rows
  → names via `open_locator`, the window around the focused file, letter
  table loaded). Otherwise: idle card task and ≤ 128 entries → built at
  once (second scratch pair, no unsorted flash); else the on-disk listing
  goes out (`sorted: false`) and the folder becomes the background build
  target (one at a time; a new target replaces the old one, whose partial
  file and scratch files are removed before the next build starts).
  Background building (`audio::build_slot`) runs only in the idle loop:
  steps of 64 entries/rows on the last 16 KiB of the decoder arena, a
  sleep as long as the step took (≥ 2 ms, thread mode keeps ≥ half the
  CPU), suspended (`Suspended` in the server) as soon as any request,
  command or player request arrives. When it finishes and the folder is
  still shown, the sorted listing is published (`Data(BROWSE)`, focus
  kept). Windows of a sorted listing come from the index rows. The play
  queue follows the listing's order when the folder's index is current.
  Card hand-over drops the index state and removes a build in progress.
* Names come from the directory entries through the index rows'
  locators (one `open_locator` per row): the alternative, a full
  directory pass per window, costs the whole directory for every window
  of a large folder.
* `BrowseService::focus_hint` (new, default no-op; Files calls it when the
  focus moves or is restored): the card task publishes the window around
  that file when a listing becomes sorted, so `position()` finds it.
  `Fw`: re-browses when the filter changes (sort order, hidden files),
  asks again for the rows the screen wants when the listing arrives or
  becomes sorted (they were dropped before while the listing was
  incomplete), letter jump from `media::LETTERS` (80 runs; larger tables,
  e.g. date order of a big folder, fall back to the loaded window).
* RAM (`xtask build`, `__stack_bottom`): stack 22,976 → 21,464 B
  (−1,512 B). `SERVER` +536 B (build state with `Suspended` ≈ 320 B, two
  directory cursors, the current `Index`, orphan name), `LETTERS` 336 B,
  focus hint 12 B, UI task future +32 B, audio task future unchanged
  (6,192 B; the 840-B `DirEntry` of a build step lives in it, not on the
  stack), ≈ 600 B resident merged strings — the panic-location paths of
  the new `orpheus-index` files (absolute paths: shorter in the main
  checkout than in a worker's worktree). Paged code +4 pages
  (`orpheus_index` 17.4 KB). Stack frames of the new sync paths: build step
  ≈ 0.9 KB + file system, window from the index ≈ 0.9 KB + `Index::rows`
  0.5 KB (served also during playback; check the stack high-water mark).
* Not done: the quick stamp (an index is only used after the counting
  pass confirms it, as before for the on-disk listing); `Index::find` for
  the focus (a linear `position_of` pass instead); several abandoned
  builds in a row leak all but the last partial file until that folder is
  built again; Music library index, playlists, Favourites (rest of AU-12).

### 2026-11-24 — AU-5 firmware: orpheus-player in the card task (worker, no hardware)

* The card task (RT executor) owns a `orpheus_player::Player` (in the
  `Server` static, built in a non-inlined function). UI commands arrive as
  `media::PlayReq` on a new `PLAY` channel, checked after every decoded
  block (and waited on while paused); media requests (`REQS`: browse, text,
  the new Up-next window, card hand-over) are still served between
  refills. Power-off moved to `PlayReq::Shutdown` so it is seen within a
  block. The decode loop feeds `Opened`, `OpenFailed`, `Position` (every
  block, source frames), `Tick` (1 s), `ReaderAtEnd`, `DecoderEnded`,
  `DecoderFailed` (10 corrupt frames, 8 underruns, failed seek, ring too
  small), `Drained`, `Shutdown`, and carries out `Open`/`OpenTrack`,
  `Seek` (decoder seek; the stream ring is now a `SeekSource`), `Pause`/
  `Resume` (DAC stopped while paused), `Stop`. `Prefetch` is not carried
  out: the next track opens after the current one played out (no gapless).
  No fades yet.
* Files on the card (formats in `orpheus_player::image`, host-tested):
  `/.orpheus/queue.bin` (magic `YQU1`, count, queue id = CRC-32 of serial and
  locators, serial, 8-byte locators, CRC-32), written when a folder is
  queued; `/.orpheus/player.bin` (588 B: resume slots A/B, 64 B each, then the
  bookmarks, 460 B, magic `YBM2`, each value with its own CRC), written in
  place (only the changed span) at pause, stop, track start, card
  hand-over and power-off — never while audio is being output (a card
  write can outlast the PCM ring), so there is no 60-s save while playing.
  Both are loaded after mount; the resume track's title is resolved and
  offered as Music → Resume.
* `orpheus-player` changes: getters (`len`, `queue_id`, `ab`, `sleep_timer`),
  bookmarks carry the sample rate (for their time in the list; `Player`
  624 → 752 B), `SleepAfterMs(0)` also cancels "after this track", a
  shuffled `PlayQueue` picks a seed whose order starts at the chosen item
  (so every item plays once), `StoreImage`/`Loading` (resume slots only; the
  bookmarks are encoded from the player when the file is written), queue
  file encode/decode. 18 tests (4 new).
* RAM (`xtask build`, `__stack_bottom`): stack 24,752 → 22,968 B
  (−1,784 B). Of that, +1,796 B is resident MP3 seek code
  (`Mp3Decoder::find` 1,142 B, `record` 166 B, more `parse_head`, rodata),
  resident through the `orpheus_codec_mp3::*` rule — it could be paged by
  excluding those symbols in `resident.toml` (not mine to edit). Data and
  bss net −16 B: the player (752 B) and resume slots went into `Server`,
  the text-viewer buffer (1,536 B) moved out of it into the decoder arena
  (the decoder now leaves 2 KiB of the 64 KiB arena free while it plays:
  largest need 59,488 B still fits), the two DSP scratch buffers became one
  (−1 KiB), Up-next and bookmark windows (2 × 208 B, 2 × 456 B), `PLAY`
  channel 144 B. Audio task future unchanged (6,192 B); `play_file` poll
  frame 2,276 → 2,372 B; UI task future +128 B.
* Open: on-device test (user check 13 and the lead's test plan); gapless
  hand-over (reader into the next file, decoder re-open in the arena);
  fades; periodic resume saves while playing; queue edits / Play next /
  Add to queue / Favourites / Shuffle all; "Resume at start-up" setting;
  OK-hold Stop clears the resume point as 05 §7.1 says (the player keeps
  its current track, so OK on an idle Now Playing still restarts it).

### 2026-11-24 — D-10 decided by the owner

* The presenter remote is a Bluetooth remote (the module's keyboard +
  consumer HID reports). The tilt (IMU) pointer is built only if BT-3 finds
  a Bluetooth transport for mouse data; otherwise SENS-2 and the pointer
  parts of UI-6b are dropped (no USB pointer, no module firmware work).

### 2026-11-24 — published (D-11, owner's decision)

* The repository is public as `github.com/timschmidt/orpheus` (branch
  `main`; worker branches stay local). Before publishing: tracked files and
  history checked for secrets, flash dumps and vendor material (none; the
  vendor-derived `docs/hw/`, `docs/vendor/` and `local/` stay ignored per
  D-7); a stray 5.6 MB test recording (`44100`, committed by mistake in the
  Bluetooth volume commit) removed from main's history; root-level audio
  files now ignored. Licence (D-1) is still open, so the code is public but
  not yet under an open-source licence.

### 2026-11-24 — AU-5 gapless/queue merged; first device run (lead)

* Merged AU-5 (gapless, queue editing). Fixed the `orpheus-img` "fixture
  race": test threads of one binary shared `gen-<pid>`; fixtures are now
  generated once per process (`OnceLock`), verified from an empty folder.
* On the player (Bluetooth off): the corpus MP3 album split from Files →
  OK on part 0 — part 0 clean, then **6 underruns after the first join and
  1 more later**, and a fourth track with part 1's length appears to play
  after part 2. Gapless is therefore **not yet working on the device**; sent
  back to the AU-5 worker with a request for a `playdir` developer command
  and queue position/title in `status` for HIL tests.

### 2026-11-24 — UI-6a merged and tested on the player; Bluetooth volume policy changed (lead)

* Merged UI-6a (Bluetooth app). On the player: Home → Bluetooth → on →
  "Starting…" → automatic reconnect → "Connected · fedora #2" (screenshots
  checked).
* **Test harness fixed**: `pw-record --target bluez_input…` can silently
  record the wrong thing (the BlueZ input is a playback *stream* in
  PipeWire). Reliable capture: a null sink (`pactl load-module
  module-null-sink sink_name=orpheus_bt_cap`), `pactl move-sink-input` of
  the `bluez_input` stream into it, `parecord -d orpheus_bt_cap.monitor`.
  This also keeps test tones off the workstation's speakers. BlueZ as sink
  starts the transport at volume 0 and accepts no absolute volume from this
  module (`EOPNOTSUPP`); the harness raises it locally to 127.
* **Volume policy changed** (06 §4.2 as built): the module acknowledges 0x26
  whether or not the sink scales, answers 0x25 with 128 for a sink without
  absolute volume, and echoes the sent value back as a sink-volume event —
  so whether a sink scales cannot be told. With UI-6a's fixed-reference
  scheme the volume keys had **no effect** on such a sink (measured: −26.5
  dBFS at every volume). Now the DAC always plays at the player's volume,
  the sink is sent full scale (127), and sink volume events no longer move
  the player's volume (following the echo would jump to full scale).
  Measured: DAC 0x6F → −53.6 dBFS, 0x57 → −62.6 dBFS, 0 → silence. Revisit
  with a real headset (user check 17).
* Developer command `bt vol N` (module-scale volume the task sends).
* Key names for `xtask dev keys` are `volup`/`voldown` (not `vol+`).

### 2026-11-24 — plan: M12 documentation/website/branding; presenter remote with tilt pointer (lead, owner's request)

* New plan document `docs/plan/09-docs-website.md` and milestone **M12**
  (BRAND-1, DOC-2…4, WEB-1…3): concise docs for users and developers,
  open-source project files, GitHub Pages site built from the repository,
  Orpheus in the browser (the real UI/app/codec crates on the simulator back
  ends, compiled to WASM), a WebUSB browser installer with backup-first
  safety rules, and one Orpheus/lyre brand generated into firmware, docs and
  site. DOC-1 moved there; REL-1 depends on M11 and M12. New decision D-11
  (public repository name, Pages address, when to publish).
* **Presenter remote and IMU (tilt) mouse back in scope**: 06 §4.3 rewritten
  (staged HID transport — the module's SDP already lists HID 0x1124; the
  tilt-pointer engine with clutch, relative/absolute/scroll modes, one-euro
  filter, jerk rejection, trace tests), 05 §7.12 Remote app (Connect,
  Present with timer and slide counter, Pointer, profiles for PowerPoint,
  Keynote, Google Slides, Impress, PDF, Media, Custom), WPs BT-3
  (investigation, now with stop/go), BT-4, SENS-2, UI-6b; decision D-10 for
  the fallback transport (USB HID, reduced remote, or module firmware).
  PLAN.md requirements, limits and non-goals updated. Note: the running
  Bluetooth-screens worker's package is UI-6a (it was briefed as "BT-3 (UI)").
* Disk: the workstation's disk was at 953 MB free; build caches of the main
  checkout and finished worktrees removed (6.8 GB free afterwards).

### 2026-11-24 — Bluetooth end to end with a host A2DP sink; amplifier gain fix (lead)

* The workstation's Bluetooth adapter served as a test headset: a BlueZ
  agent (auto-accept, discoverable for a limited time) and PipeWire's A2DP
  sink, recorded with `pw-record`. On the player (test mode): `bt on`,
  `bt scan` found "fedora #2", `bt connect 0` paired and connected (no PIN),
  `bt on` later auto-reconnected to it, the module streamed SBC; the module
  appears to the host as `F4:4E:FD:06:7A:20` "ZS285AH_TARNS" with A2DP,
  AVRCP, HFP, **HID (0x1124)** and PnP records.
* **Bug found and fixed: the analogue output was practically muted.**
  `dac::power_up` never set the PA volume field `PA_CTL[14:12]` (vendor: 7).
  Measured through the module's tap: PA 0 → silence, PA 7 → the 522 Hz test
  tone at the reference level ±1.06 dB (DAC 0 dB) and −53.6 dBFS at the UI
  volume (DAC −30 dB) — the tap is after the PA and after the volume
  (U-BT-2 partly answered). This most likely also means the wired
  headphones were near-silent before this build; user check 9 (tones) must
  be made on this build or later.
* Found for BT-3 (sent to that worker): the firmware never sends absolute
  volume (0x26), and the sink starts at 0 → silent stream until it does;
  addresses are shown byte-reversed (`to_text`); connect is refused during
  an inquiry.
* Tools: `xtask dev mem ADDR [N]` reads words from the running firmware over
  the developer channel (`dev peek` switches the player to ymon); PEEK now
  reads aligned requests as 32-bit words (the DAC block reads 0 by byte);
  developer command `pa N` sets the PA volume field.

### 2026-11-24 — plan: final verification stage M11 (lead, owner's request)

* Added milestone **M11 Verification** (08: VER-1…VER-11; 07 §3.5–3.8 sanitizers,
  coverage, adversarial inputs, host UI-sequence fuzzing; §4.4 memory and
  performance history; §5.7–5.9 on-device interaction/fuzz testing, device
  coverage, sanitizers for device code; PLAN.md §4 item 11, decision D-9 for
  coverage exclusions; definition of done extended). Fuzz findings are kept
  in the corpus permanently.
* RTC: measured against the host, 0 s off after 30 s and after an ADFU
  round trip + reset, and it kept running through a firmware reboot to ADFU,
  a RAM start and a watchdog reset; the earlier stop is not reproduced
  (open, U-RTC). The status bar now hides the clock while it is not running.

### 2026-11-24 — AU-12 library on the player (lead)

* Merged AU-12 (firmware, library) (worker). On the player with the owner's
  card (5,740 audio files): the first library build ran in the idle loop
  and finished after ≈ 11 min (estimate was 1–2 min: tag reads per file are
  slower than assumed — open: measure per-file cost, consider reading only
  the first 4 KiB for files whose ID3v2 header shows no text frames); the
  UI stayed responsive throughout and the stack never fell below 6.4 KB
  free. Albums lists the folder names; Artists shows only "Unknown artist"
  — correct: the owner's files carry no title/artist/album tags (only
  `encoder=Lavf61.7.100`, checked read-only through the card reader).
* RTC found stopped again after a series of test resets (clock showed
  00:00; `dev time` reported not running); `dev time --set` restarts it.
  Open (U-RTC): which reset path stops the calendar, and show "clock not
  set" instead of 00:00.

### 2026-11-24 — AU-12 library (host part) merged (worker)

* `orpheus-library`: on-card music library under `/.orpheus/library`
  (two slot files, header written last; incremental updates from per-folder
  signatures; tags read with a budget, ≈ 5.7 KB / 2.4 reads per file on a
  realistic mix), lists Artists → Albums → Tracks, Albums, Songs, Genres →
  …, Recently added; queues and a uniform Shuffle-all sample. Additions:
  `orpheus-index::ExtSort`, `orpheus-meta` budgeted reads and durations.
  Host: ≈ 52 k tracks/s build, 36 ms unchanged update of 5,000 tracks;
  ≈ 200 KB on the card per 1,000 tracks; permanent build state 1,088 B.
  Not yet in the firmware; estimate for the owner's 5,740 files: ≈ 33 MB of
  tag reads, first build 1–2 min (per-file seek cost unknown). Ogg/MP4 tags
  not read yet (names from files/folders).

### 2026-11-24 — project renamed to Orpheus (lead, owner's request)

* "Orpheus" → **Orpheus** everywhere (docs, plan, code, configuration,
  USB strings, build id `orpheus-fw`, xtask state name); crates `orpheus-*`
  → `orpheus-*` except `yoton-bsp` (board support for the Yoton YM03 —
  owner's choice); `orpheus_*` symbols, linker symbols, section names and
  environment variables (`ORPHEUS_BLESS`, …) renamed; `tools/orpheus-ld`,
  `tools/udev/99-orpheus.rules`, output directory `target/orpheus/`. Not
  renamed: the hardware ("Yoton YM03"), the reference repository
  (`/home/tim/Documents/GitHub/yoton/`, `yoton.py`, payloads), the on-flash
  magics (`YST2`, `YFW1`, … — stage 1 in sector 0 checks `YST2`), and this
  checkout's folder name (`…/GitHub/yotonrs`; renaming it is up to the
  owner).
* Card folder `/.orpheus/` → `/.orpheus/` (owner's choice: move the old
  data). The firmware renames it once at mount (write guard lifted for that
  single rename via the new `Volume::set_write_prefix`); on the player the
  queue, resume record, indexes, art cache and test files moved over.
  First version remounted twice and gave `mount` a 10.5 KB stack frame (744 B
  of stack left after boot); now a single mount.
* Logo: a lyre (owner's request) — `docs/logo/orpheus-lyre.svg` (README)
  and the 24×24 `LYRE` icon on the boot/About screens (goldens re-blessed).
* Stack: `Fw`, the settings store and `Ui` (≈ 5 KB) now live in statics
  built by non-inlined constructors; the UI task's poll frame went 6.0 →
  4.7 KB and the stack grew to 27.8 KB (the task future shrank). On the
  player after boot + browse + play + view changes: 7.2 KB free (was 2.8
  KB). The audio task's 5.1 KB frame is the next candidate. The
  orpheus-img test fixtures race when generated fresh by several test
  binaries at once (passes when run alone).

### 2026-11-24 — implementation session 3 (lead)

* Merged AU-14 (device): Pictures app and album art. On the player: Home
  shows Pictures; Show hidden → `/.orpheus/test`: a 1600×1200 JPEG letterboxed,
  a truncated JPEG shows "Cannot show this picture", BMP and PNG render, Down
  rotates by 90°. Album art on the player: an MP3 with an embedded JPEG cover
  (uploaded to `/.orpheus/test/`) shows it in Now Playing's Big art view; the
  first open of a track with new art costs underruns (9 here) while the
  cover is decoded — open item: decode art without stopping the DAC feed, or
  before the previous track ends. The owner's album files showed no art
  (they may have none embedded).
* The "garbled" scrolling text in screenshots was a screenshot artifact: the
  frame buffer is read in 1 KiB pieces while the UI keeps redrawing the
  marquee row. The UI now holds drawing while a screenshot is read (≤ 300 ms);
  four shots of a scrolling row are clean.
* Shuffle all: walks the card's visible folders (no recursion, depth ≤ 8)
  with reservoir sampling into the 256-item queue, then plays shuffled. On
  the player: 5,794 entries / 5,740 audio files in ≈ 27 s, UI responsive,
  then playing. The walk is capped at half the CPU (sleep as long as each 8
  entries took); a fixed 1 ms per 16 entries starved the UI into the
  watchdog after 2,061 entries (found with walk counters in spare
  persistent words). 27 s is slow: the library index (AU-12 rest) replaces
  it.
* **Second NOR-write crash fixed**: LLVM had inlined `nor::program_page`
  (its busy-polling loop) into the paged `NorRegion::program`, so the
  resident-root rule protected only the unused out-of-line copy; toggling
  Show hidden crashed at a line boundary (UsageFault UNDEFINSTR at
  0x231000). All NOR entry points are now `#[inline(never)]`; no paged code
  calls `nor::status` any more. `check-resident` cannot see inlined copies:
  any function that must run resident while the chip is busy needs
  `#[inline(never)]` as well as a root.
* Stack: with Pictures merged only 872 B of 23.1 KB stayed free at the peak
  (picture decoding in the card task over the UI). The USB control buffer
  went from 4 to 1 KiB (the host now moves ≤ 1 KiB per control transfer):
  stack 26.2 KB, 3.9 KB free after pictures + playback. Follow-up: shrink
  the UI task (6.0 KB frame) and card-task frames (5.1 KB; 5.7 KB during card
  hand-over).

* Resumed the two interrupted workers from WIP snapshots. Merged BT-2
  (device). On the player: `bt status` (off at boot), `bt listen` (module TX
  pad toggles 13×, RX pad quiet: wiring as documented), `bt probe` (identity
  "ATS2851", 37 bytes / 5 frames, 0 bad checksums, 3 commands acknowledged,
  DMA RX ring works → U-BT-4 answered), `bt on`/`bt off`, `bt scan`
  (inquiry runs; nothing found in 35 s — needs a headset in pairing mode,
  user check 14), `bt shutdown` (0x24 sent and acknowledged, module off).
* Budgets updated for the full layout; A–B repeat and shuffle verified.

### 2026-11-24 — implementation session 2, part 8 (lead)

* On the player: bookmarks (Now Playing menu → Bookmark here; Music →
  Bookmarks lists it; choosing it plays from the position), resume, shuffle
  toggle (the queue then plays out of folder order: 27 / 231 after the
  resumed track), A–B repeat (markers on the bar; elapsed time cycling
  0:12 → 0:15 → 0:09 → 0:12 … between the marks). "Shuffle all" is not
  implemented (opens an idle Now Playing). Sleep timer not yet observed.
* **Fixed a crash in NOR writes** (02 §3.2 rule 5): toggling Shuffle wrote
  the settings store; `nor::status`/`wait_ready` and the command-byte
  constant were paged, so the pager loaded them while the chip was busy
  programming and the CPU executed garbage (UsageFault UNDEFINSTR in
  `nor::status`). Now `atj2157_hal::nor::*` is a residency root (code and
  constants), and program/erase run with interrupts masked (BASEPRI) so no
  fault can reach the busy chip (suspend/resume is U-NOR-2). Five toggles
  during playback: no crash, 0 underruns. Sector erases (settings log
  roll-over) hold interrupts off for tens of ms; the 70 ms PCM ring covers
  it.

### 2026-11-24 — implementation session 2, part 7 (lead)

* Merged AU-12 firmware (worker): Files lists through `/.orpheus/index/`. On
  the player: the root (35 entries) is sorted at once; a 231-entry folder
  shows on-disk order first and re-sorts in the background within ~20 s;
  playing from it, seeking and browsing while playing work.
* MP3 open/seek code (`Mp3Decoder::find`, `parse_head`, `parse_vbr_tag`)
  excluded from residency: resident −2.5 KB, stack 24.9 → 27.4 KB (peak
  ≈ 21.5 KB measured after browse + play + seek).
* Browsing during playback caused up to 6 underruns (counting pass + index
  open between refills), close to the too-slow abort (8). Underruns within
  500 ms after a served request no longer count towards the abort; Opus is
  still detected as too slow.

### 2026-11-24 — implementation session 2, part 6 (lead)

* Merged AU-12 host (`orpheus-index`, worker) and AU-5 firmware (orpheus-player
  driving playback, worker). On the player: Files → folder → OK on track 2
  plays with "2 / 231"; Next → "3 / 231"; OK pauses; `/.orpheus/queue.bin`
  and `/.orpheus/player.bin` written; after a normal reboot Music offers
  "Resume ‹title›", which continues the same track at the paused position;
  holding Next seeks. Fixed: the DAC kept running during a seek (7
  underruns per seek, close to the too-slow abort); it now stops for the
  seek (3 seeks: 0 underruns).

### 2026-11-24 — implementation session 2, part 5 (lead)

* Upload path for test files (user-approved): developer-channel `PUT`
  request (≤ 4 KiB per transfer, buffer in the idle decoder arena),
  `fcreate`/`fclose` commands, `xtask dev put LOCAL /.orpheus/PATH`; writes go
  through orpheus-fs (write prefix `/.orpheus`). ≈ 50 KB/s. Uploaded Vorbis,
  Opus (1 s and 0.25 s pages) and Speex test files to `/.orpheus/test/`.
* Results above; codec clock floors (Vorbis/Opus: High) set by the audio
  task. Found: the output CRC32 (bitwise, per frame) cost a large share of
  48 MHz; now only with `crc on` (MP3 at 48 MHz: ≈ 87 % idle). A `NeedData`
  the full ring cannot satisfy now ends the track instead of spinning. A
  `REQ_PUT` arm compiled as a catch-all binding (missing import) — caught by
  the orpheus-usb tests; `xtask build` does not run clippy, the gate does.

### 2026-11-24 — implementation session 2, part 4 (lead)

* DSP hang solved. Findings on the way: the audio task's poll function had
  a 13.6 KB stack frame (a 4 KB `Chain` built by value inside it, plus
  futures built on the stack); `Chain` is now built in `main` (frame 4.5 KB,
  start-up stack use 23 → 14 KB). A PC sampler in the IRQ dispatcher
  (persistent words `0x137FE4/E8`, copied to `0x137FDC/E0` at boot) showed
  the RT executor in the PCM push loop: at 48 MHz, MP3 + DSP + the paged
  push loop and DSP ramp helpers overload the CPU and starve thread mode
  until the watchdog fires (the chain alone runs at ≈ 20 % on the player,
  `dspt` command; FP inside the RT executor is fine, `fp` command). Fix: a
  96 MHz clock floor while the chain is active during playback (set by the
  audio task, respected by the governor). The DSP also works on a scratch
  copy of the decoder output. Test commands `dsp`, `gov`, `clock`, `dspt`,
  `fp`, `sound`.

### 2026-11-24 — implementation session 2, part 3 (lead)

* Audio hot path resident (`resident.toml`: `orpheus_codec_mp3::*` and the
  DSP stages' `process` functions), stream ring 32 → 16 KiB, decoder arena
  72 → 64 KiB (largest measured need: Opus 59,488 B). MP3 at 48 MHz now
  ≈ 80 % idle (was ≈ 53 % with the MP3 cluster, ≈ 0 % before), 0 underruns,
  all codecs linked; `mp3-decode` hot cluster dropped (resident instead).
* After an underrun the PCM push now waits for the next DMA half, so a
  decoder that cannot keep up still yields (the RT executor no longer
  starves thread mode).
* **Open: DSP chain hangs on the player.** With any single stage active
  (bass, crossfeed or EQ) playback stops inside `Chain::process` (breadcrumb
  5) until the watchdog fires; also with all of orpheus-dsp resident, so not
  paging. The same sequence (`set_format` 44.1 kHz i16 stereo, flat, then
  the stage) runs in milliseconds on the host. Next: run the chain in thread
  mode on the player (needs RAM: start-up uses 23–28 KB of the 32 KB stack,
  which should be found and cut first), check FP in handler mode, compare
  the thumbv7em code of `Chain::run` with the host's. The chain is built in
  and fed by the Sound settings but bypassed (`DSP_ON` false; test command
  `dsp on`).
* `clock 48|96|144` test command.

### 2026-11-24 — implementation session 2, part 2 (lead)

* User decisions: D-2 full layout (done), BT 0x24 in power-off, Opus seek
  tolerance accepted, test files on the card allowed.
* DSP chain wired (UI → `media::Sound` → audio task); `sound` and `dsp
  on|off` test commands; persistent breadcrumb word `0x137FEC` for
  watchdog post-mortems. Found and fixed: unsigned compare in the PCM ring
  wait deadlocked playback after an underrun (DMA looped stale audio).
* With Opus and Speex linked, MP3 decoding thrashed into the watchdog;
  `hot.toml` cluster `mp3-decode` (8 lines) fixes it: 882,000 frames, 0
  underruns, ≈ 53 % idle, ≈ 1,060 line loads/s at 48 MHz. DSP active still
  thrashes (see AU-6); lazy FP stacking switched off (FPCCR.LSPEN = 0) on
  the way, kept as the safer setting for FP in handlers.

### 2026-11-24 — implementation session 2 (lead)

* Paged firmware boots from NOR; Home, Files, Music, Settings on the player.
  Hang hunted down with new STATS counters (pager, UI loop, RT phases, IRQs
  per line): at 48 MHz the Now Playing UI (~35 %) plus MP3 decode with page
  refaults (~56 %) starved the USB task; the governor (96 MHz) fixes it
  (46 % idle while playing from Files). Also fixed: main-stack overflow (Fw
  held 6 KB by value), card-reader buffer lent from the decoder arena (+16 KB
  stack), handlers bound through `rt::bind` placed resident, a browse
  request loop for short folders, watchdog fed by the pager and every
  executor poll (now: watchdog task on RT + main + UI progress).
* **Full-flash layout (D-2, user decision)**: new stage 2 (slots tried in
  order, slot index recorded, stock path removed), firmware finds its slot,
  HAL NOR guard follows the layout, Rust NOR payload, `install-full`,
  `restore vendor`. Sequence run: dump whole chip (`local/nor-before-full.bin`,
  differs from the stock backup only in sector 0 bytes 0–0x1FF and above
  0x762000), stage 2 tested from RAM, installed, booted; firmware written to
  0x1000 (10.2 s), 0x63000–0x7DA000 erased (1,907 sectors, 35.6 s); boots;
  the 544,768-byte image with Opus and Speex (too big for the dual slot)
  boots from full slot A.
* Merged worker results: OTP-1, BT-1, USB-6, AU-5 (`orpheus-player`), AU-7,
  AU-6, UI-3/UI-4, AU-14, AU-8/AU-9 with the Ogg layers unified.


### 2026-11-24 — AU-8/AU-7 Ogg reconciliation (worker)

* AU-7 (Vorbis, on main) and AU-8 (Opus, Speex) each wrote a `orpheus-ogg`.
  Merged into one crate with a shared core and two packet readers:
  * core (`lib.rs`, `seek.rs`): `PageHeader::parse`, `crc_update` /
    `page_crc`, page sync, the generic answer `Next<P>` (`Packet`,
    `NeedData`, `End`, `Corrupt`), and `Bisect` (granule bisection over a
    `SeekSource`, now holding the stream serial; `Bisect::step` replaces
    `PacketReader::seek_step`).
  * `ranges::Reader` (AU-7's, ported onto the core): zero-copy packets as
    byte ranges of the ring, pages larger than the ring, oversized packets
    skipped as they stream past, `peek_packets` for inspection. Used by
    Vorbis.
  * `assembled::PacketReader` (AU-8's, unchanged apart from the module):
    packets copied into a caller buffer, serial selection, chained-stream
    hand-over, gap detection, resume at a page's granule. Used by Opus and
    Speex.
* Vorbis seeking now uses the shared `Bisect` instead of its own loop
  (granule-0 pages are mapped to "start at the first audio packet", as
  before); AU-7's `find_page`/`page_at`, used only by that loop, are gone.
* The `ogg` fuzz target drives both readers over the same input.
* Conformance after the merge (`xtask test codecs --gen`, 226 files, 0
  failed): Vorbis −114.7 … −122.3 dBFS, 23/23 seeks exact on every file,
  album join −121.2 dBFS (unchanged); Opus corpus −88.4 … −138.4 dBFS,
  RFC 8251 vectors all 12 (24 decodes) range-exact, ≤ −128.9 dBFS
  (unchanged; SILK seek tolerance −50 dBFS kept as AU-8 set it, user
  decision still open); Speex −99.9 … −119.6 dBFS, max 4 LSB (within the
  AU-9 figures).
* Code size (thumbv7em, opt-level s, before LTO): `orpheus-ogg` 7.7 KB for
  both readers and the bisection (AU-8's alone was 4.8 KB). `AnyDecoder`
  on thumbv7em: 984 B (Vorbis variant; Opus 784 B, Speex 608 B).

### 2026-11-24 — AU-9 Speex (worker)

* `crates/codecs/speex` (`orpheus-codec-speex`): libspeex 1.2.1 decoder
  (BSD-3, `LICENSE-XIPH-SPEEX`) in `f32` (float build; no `f64`), NB/WB/UWB,
  every sub-mode, vocoder/null modes, DTX, enhancer on (libspeex default,
  as ffmpeg leaves it), intensity stereo, any frames per packet, native
  8/16/32 kHz output. Uses `orpheus-ogg` unchanged.
* Conformance (31-file corpus, references from ffmpeg `-c:a libspeex`):
  −97 … −120 dBFS RMS, max 5 LSB, lengths exact. Length convention: ffmpeg
  trims nothing for Speex (granules start at −look-ahead, every frame is
  output), so neither does the decoder (`StreamInfo.frames` is `None`).
  Seek: skims packets to the pre-roll start (for streams ≤ 1 MiB, else Ogg
  bisection), then decodes 64 frames (1.28 s) of pre-roll; all corpus seeks
  exact or within −80 dBFS. Beyond 1 MiB, noise-excited frames after a seek
  carry different (statistically equal) noise.
* Arena (used = plan; mono and stereo equal): NB 4,872 B, WB 7,768 B,
  UWB 11,048 B of the 12 KiB budget (packet buffer 2 KiB). Code
  (thumbv7em, before LTO): 32.9 KB. Deepest stack ≈ 1.5 KiB.
* Not done: device benchmark, HIL CRC test, `hot.toml` cluster, chained
  streams.

### 2026-11-24 — AU-8 Opus (worker)

* `crates/codecs/ogg` (`orpheus-ogg`): page parsing, CRC (checked before use
  for pages ≤ `MAX_PEEK`, streamed for larger ones), packet reassembly into
  a caller buffer, serial selection, chained-stream hand-over, granule
  bisection seek. Written here because AU-7 had not created one yet: the
  lead reconciles with AU-7.
* `crates/codecs/opus`: port of the libopus 1.5.2 decoder (BSD-3, licence in
  `LICENSE-XIPH`). SILK integer (bit-exact; every libopus build decodes SILK
  in fixed point), CELT in `f32` because the conformance references
  (ffmpeg's libopus, the RFC 8251 `.dec` files) are float builds and the
  M4F has a single-precision FPU; no `f64`, no recursion (`quant_partition`
  runs on an explicit stack), `sqrt` correctly rounded, `exp2`/`cos`
  within 1–2 ulp. `opuscule` was evaluated and not adopted: MPL-2.0 (not
  permissive under D-1) and a by-value decoder state outside the arena.
* Conformance: all 12 RFC 8251 vectors, stereo (`.dec`) and
  phase-inversion-disabled (`m.dec`, RFC 8251 §10): final range equal on
  every packet, RMS difference ≤ −128.9 dBFS, max 1 LSB (SILK-only vectors
  bit-exact). Corpus (`opus/`, 42 files, libopus via ffmpeg and opusenc,
  SILK/hybrid/CELT, 2.5–120 ms frames, CBR/VBR, header gain ±6 dB, picture
  in OpusTags): −88 … −138 dBFS, lengths exact (pre-skip and end trimming),
  surround refused. Seek: CELT/hybrid exact after the 400 ms pre-roll; SILK
  on stationary synthetic tones keeps −57 … −78 dBFS for seconds (its
  integer LPC/LTP state converges slowly), accepted by a separate Opus seek
  tolerance of −50 dBFS in `checks.rs` (a deviation from 07 §3.3 for the
  lead to confirm).
* Arena high-water (stereo / mono): 58.1 KiB (59,488 B) / 42.1 KiB of the
  64 KiB budget (packet buffer 4 KiB; larger packets are concealed).
  Code (thumbv7em, opt-level s, before LTO): opus 117.6 KB, ogg 4.8 KB.
  Largest stack frames: decode chain ≈ 3 KB (no frame above 1.2 KB).
  Host speed (conformance build, overflow checks on): 26–273 ns per sample
  frame, i.e. 80–800× real time.
* Not done: device benchmark (04 §4.3 row), HIL CRC test, `hot.toml`
  cluster, multistream/surround, FEC, Opus in other containers.

### 2026-11-23 — implementation session 1 (lead + four host-track workers)

* M0 tooling and M1 bring-up images on the player (all RAM-loaded; Rockbox
  untouched in NOR). Each bring-up image is a HIL case:
  `cargo xtask dev hil smoke` (hello, deliberate fault) and
  `cargo xtask dev hil drivers` (lcd, keys, charge, sd, audio, i2c).
* Hardware facts learned (also for 01-hardware §9):
  * BASEPRI must be lowered after init: start-up masks levels 1–3.
  * Timers clocked at CK24M/24 give an exact 1 MHz tick (10 s window =
    10,000,025 µs vs host); RT ticker at 1 kHz: worst lateness 14–21 µs.
  * Panel id `7C 89 F0` (ST7735S); DMA blit with `IN_BUS=1`,
    `RGB565_FORMAT=1` streams the little-endian frame buffer unchanged:
    2.77 ms per full screen at LCD clock CK24M with 1+1 strobe clocks; GRAM
    read-back (RAMRD) works.
  * Key ladders idle at 743 / 837 (noise ±3).
  * Charger: `CHG_CTL` reset state `0x8C40073A` (off, CV 4.1875 V, stop
    4.18 V); charging `0x8C450738` (code 4); `CHG_DET` `0xB8` = CC phase.
  * DMA: half-transfer interrupt needs `DMAIE` bit n+16; reload mode works
    (U-DMA-2); DMA reads SRAM3 through `0x18000000` (U-DMA-1).
  * DAC sample clock measured ≈ +0.15 % fast at 44.1 and 48 kHz against
    T0 (open: audio PLL value vs measurement).
  * I2C: devices at 0x10/0x11 (tuner, also 0x60), 0x27 (accelerometer);
    tuner id 0x5804, accelerometer id 0x13; lines idle high.
  * SD: with no card, CMD8 gets "no response" (STATE bit 4). With the
    card: init at the port's clocks, reads at 9.3 MB/s.
  * Stack overflow is silent without the MPU guard (a 64 KiB temporary on
    the stack jumped to address 0): PG-1 must add the guard region.
* Vendor font: `UNICODE.FON` glyph table at offset `0x172C00` of
  `nor_descrambled_8m.bin` (directory entry at `0xFC00`, length `0x210000`);
  33-byte records verified, baseline row 13 (UI worker).
* Scope changes from the owner, applied to the plan: no Bluetooth receiver,
  no mouse, no Remote app.

### 2026-11-23 — planning session

* Read the research repository and the Rockbox port; four static
  reverse-engineering studies of the stock firmware and one ecosystem study;
  found the vendor datasheet. Nothing was run on the player.

## 4. Measured numbers (append; never overwrite history)

| Date | What | Value | How |
|---|---|---|---|
| (Rockbox port, 2026-11) | NOR line read, 1 KiB, SPI 48 MHz, DMA | ≈ 180 µs | FINDINGS §13 |
| (Rockbox port, 2026-11) | 320 kbit/s MP3 with UI at 48 MHz | ≈ 85 % CPU | FINDINGS §13 |
| (Rockbox port, 2026-11) | USB card-reader read speed | ≈ 2.6 MB/s | FINDINGS §13 |
| (Rockbox port, 2026-11) | Resident image / page store | ≈ 76 KiB / 48 pages | FINDINGS §13 |
| 2026-11-23 | T0 window vs host | 10,000,025 µs in ≈ 10.0 s | `hil smoke/hello` |
| 2026-11-23 | RT executor 1 kHz ticker lateness | max 14–21 µs, mean 14 µs | `hil smoke/hello` |
| 2026-11-23 | Main executor idle share at rest | 99.9 % | `hil smoke/hello` |
| 2026-11-23 | LCD full-screen DMA blit / PIO fill | 2.77 ms / 6.85 ms | `hil drivers/lcd` |
| 2026-11-23 | DAC consumption at 44.1 / 48 kHz | 44,168–44,229 / 48,076 Hz | `hil drivers/audio` |
| 2026-11-23 | SD sequential read, 64 KiB / 4 KiB requests | 9,294 / 3,432 KiB/s | `hil drivers/sd` |
| 2026-11-23 | Battery raw at rest / charging | 801 / 828 (≈ 3.74 V) | `hil drivers/charge` |
| 2026-11-24 | Opus vs libopus float, RFC 8251 vectors (24 decodes) | range-exact; RMS ≤ −128.9 dBFS, max 1 LSB | `cargo test -p orpheus-codec-opus --release` (`OPUS_VECTORS`) |
| 2026-11-24 | Opus arena high-water stereo / mono | 59,488 B / 42.1 KiB (budget 64 KiB) | `xtask test codecs --filter opus/` |
| 2026-11-24 | Opus code size thumbv7em (before LTO) | 117.6 KB (+ orpheus-ogg 4.8 KB) | `xtask test codec-size` |
| 2026-11-24 | Speex vs libspeex (ffmpeg), 31 files | −97 … −120 dBFS RMS, max 5 LSB | `xtask test codecs --filter speex/` |
| 2026-11-24 | Speex arena NB / WB / UWB | 4,872 / 7,768 / 11,048 B (budget 12 KiB) | same |
| 2026-11-24 | Speex code size thumbv7em (before LTO) | 32.9 KB | `xtask test codec-size` |
| 2026-11-24 | orpheus-ogg code size thumbv7em (before LTO), both readers + bisection | 7,673 B | `xtask test codec-size` |
| 2026-11-24 | Vorbis / Opus / Speex after the Ogg merge | −114.7…−122.3 / −88.4…−138.4 / −99.9…−119.6 dBFS | `xtask test codecs --gen` |
| 2026-11-24 | MP3 48 kHz album file via `dev cmd play`, Home screen, 48 MHz | real time, 0 underruns, main idle 52 %, ≈ 790 NOR line loads/s | `xtask dev stats` |
| 2026-11-24 | Same file started from Files (Now Playing), 48 MHz | main idle ≈ 0 %, decode ≈ 56 %, UI draw ≈ 35 % → USB starved | `xtask dev stats` |
| 2026-11-24 | Same, governor at 96 MHz | 0 underruns, main idle ≈ 46 % | `xtask dev stats` |
| 2026-11-24 | Pager line loads at rest (Home) | ≈ 240/s | `xtask dev stats` |
| 2026-11-24 | Main stack: size / used at idle | 32,192 B / ≈ 14 KB (full image) | INFO stack high-water |
| 2026-11-24 | Watchdog reset after a blocked main executor | ≈ 3 s | `xtask dev cmd hang` |
| 2026-11-24 | Card reader sequential read (write-protected test mode) | 2.3 MB/s | `dd` 128 MiB |
| 2026-11-24 | NOR program via Rust payload / erase of 1,907 sectors | 401,408 B in 10.2 s / 35.6 s | `xtask dev install-full` |
| 2026-11-24 | Firmware image, all codecs (full layout) | resident 54,308 B, 43 code + 16 rodata pages, slot 544,768 B | `xtask build` |
| 2026-11-24 | Stage 2 (Rust, full-layout aware) | 876 B of 1,024 | `cargo build -p stage2` |
| 2026-11-24 | MP3 44.1 kHz, 48 MHz, audio hot path resident | ≈ 80 % main idle, 0 underruns, ≈ 1,250 line loads/s | `xtask dev stats` |
| 2026-11-24 | Main stack after start-up (32.6 KB stack) | 9.5 KB free (≈ 23 KB used); after moving `Chain::new` to `main`: 18.6 KB free (≈ 14 KB used) | INFO stack high-water |
| 2026-11-24 | DSP chain alone (bass, 44.1 kHz stereo) on the player, 48 MHz | 20 blocks × 1152 frames in 97 ms (≈ 19 % of real time) | `dev cmd dspt` |
| 2026-11-24 | MP3 + EQ + bass + crossfeed at 96 MHz | 0 underruns, ≈ 90 % idle | `xtask dev stats` |
| 2026-11-24 | MP3 44.1 kHz at 48 MHz, output CRC off | ≈ 87 % idle | `xtask dev stats` |
| 2026-11-24 | Vorbis q5 44.1 kHz stereo | real time at 144 MHz, 0 underruns | `dev cmd play` |
| 2026-11-24 | Speex WB 16 kHz | real time at 48 MHz, 0 underruns | `dev cmd play` |
| 2026-11-24 | Opus 96 kbit/s 48 kHz | not real time at 48–144 MHz (paging-bound) | `dev cmd play` |
| 2026-11-24 | Upload over the developer channel | 165 KB in 3.3 s (≈ 50 KB/s) | `xtask dev put` |
| 2026-11-24 | Main stack peak after all formats + DSP (28.0 KB stack) | 19.2 KB | INFO stack high-water |
| 2026-11-24 | Firmware with the directory index (AU-12 firmware) | resident 81,844 B, main stack 21,464 B (−1,512), 52 code + 14 rodata pages, slot 626,688 B | `xtask build`, `nm` `__stack_bottom` |
| 2026-11-24 | Firmware with orpheus-player (AU-5 firmware) | resident 80,724 B (was 79,084), bss 122,608 B, main stack 22,968 B (was 24,752), 48 code + 14 rodata pages | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with BT-2 device side (worktree build) | resident +160 B, bss +2,136 B, main stack −2,288 B; `orpheus_bt::driver::Driver` 1,328 B | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with Pictures and album art (AU-14 device), worker worktree build | resident 81,444 B (was 79,532), bss 123,160 B (was 123,000), main stack 21,696 B (was 23,768), 62 code + 16 rodata pages (was 52 + 14), slot 724,992 B | `xtask build`, `nm` `__stack_bottom` |
| 2026-11-24 | Firmware with the music library (AU-12 firmware), worker worktree build | resident 83,244 B (was 81,996), bss 123,088 B (was 123,000), main stack 19,968 B (was 21,304), 71 code + 16 rodata pages (was 66 + 16), slot 802,816 B (was 761,856); audio task poll frame 5,024 B (was 5,088) | `xtask build`, `llvm-nm` `__stack_bottom`, `llvm-objdump` `sub sp` |
| 2026-11-24 | Firmware with gapless hand-over and queue edits (AU-5), worker worktree build | resident 83,252 B (unchanged), main stack −312 B (`__stack_bottom` 0x131c00 → 0x131d38); audio task poll frame 5,024 → 4,992 B, `play_file` 2,380 → 2,332 B | `xtask build`, `llvm-nm`, `llvm-objdump` `sub sp` |


## 5. Open questions for the user

* User checks 7–10 in `docs/USER-CHECKS.md` (picture, keys, charge current,
  tones). 8 and 10 are blocking for M4 and M5.
* Decisions D-1, D-3 … D-8 in `docs/PLAN.md` §5 (defaults assumed); D-2 decided (full layout).
* Answered 2026-11-24: Bluetooth shutdown command 0x24 **is** part of normal
  power-off (BT-2 adds it to the power-off path); the −50 dBFS Opus SILK seek
  tolerance is accepted; test files may be added to the card (under
  `/.orpheus/test/`).
