# Status

Living log. Workers update it at the start (package claimed) and end (result)
of every session. Newest entries first in §3.

## 1. Where things stand

| Item | State |
|---|---|
| Plan | `docs/PLAN.md`, `docs/plan/`. **D-2 decided 2026-11-24 by the user: full-flash layout** ("wipe the original firmware and take over the full player hardware") |
| Code | Host workspace (`crates/*`, `sim`, `xtask`) and device workspace (`fw/`: `atj2157-hal`, `yoton-bsp`, `firmware`, `boot/stage2`, `payloads/norprog`, `bringup/*`) |
| Player | **Runs yotonrs from NOR, full layout.** Stock firmware and the Rockbox port are erased. Card: the owner's music (exFAT/FAT, 238.3 GB), read only except `/.yoton/` |
| NOR | Sector 0: Rockbox-port stage 1 (unchanged). `0x001000`: full slot A (yotonrs, 544,768 B). `0x7DA000` settings, `0x7DE000` vault, `0x7DF400` Rust stage 2 (876 B, tries full A, full B, dual slot; no stock-firmware path). `0x7E0000–0x7FFFFF` vendor data, untouched. Backups: `yoton/dumps/nor_backup_8m.bin` (stock, verified), `local/nor-before-full.bin` (the chip just before the switch), `local/rockbox-nor-0x762000.bin`, `local/stage2-previous.bin`. Way back: `xtask dev restore vendor --i-have-a-backup` (`--verify` compares) |
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
| PWR-3a | started (device) | Power-off: card flushed, screen off, vendor PMU sequence (wake on long press or USB plug-in); in test mode a reboot instead (verified: long press → "power-off in a test: rebooting instead"). Real power-off = user check 11. Auto-off, low-battery handling, alarm wake not done |
| RTC-1 | done (device) | set/read over the developer channel (`xtask dev time [--set]`); alarm untested; once lost after a ymon reset (U-RTC open) |
| USB-4 | done (device) | Card-reader personality: card handed between the card task and USB; write-protected test mode; mid-track switch and hand-back verified. 2.3 MB/s read (open) |
| FS-1 | done | `yoton-fs` |
| FS-2 / AU-5 firmware | first cut on the player; **`yoton-player` wired in (built, not yet run on the player)** | Files lists the card (sorted through the directory index since AU-12 firmware), text viewer window, Now Playing. The card task drives `yoton_player::Player`: folder queue in RAM and `/.yoton/queue.bin`, resume record and bookmarks in `/.yoton/player.bin`, seek, shuffle/repeat (settings), A–B, sleep timer, Up next. Not done: gapless prefetch, fades, queue edits, Shuffle all. User check 13 |
| UI-1, UI-2, UI-3, UI-4 | done (host); running on the player | 457 golden images |
| AU-1…AU-4, AU-7, AU-8, AU-9 | done (host); on the player: MP3/FLAC/WAV and Speex at 48 MHz, Vorbis at 144 MHz (clock floor), **Opus too slow** | Opus CELT touches more code per frame than the 32 KB code cache holds; ends with "decoder too slow for this player" (abort after 8 underruns) instead of a reset. Needs a bigger code cache or a resident hot path (RAM decision). Ogg pages over ~15.5 KB fail (16 KiB ring < `MAX_PEEK` 28 KiB; RAM) |
| AU-6 | done (host); **working on the player** | `yoton-dsp` chain between decoder and DAC, fed by the Sound settings; works on a scratch copy of the decoder output; while any stage is active during playback the clock floor is 96 MHz (EQ + bass + crossfeed: 0 underruns, ≈ 90 % idle). WSOLA not done; Levels meter not wired |
| AU-12 (directory index) | host done; **firmware wired in (built, not yet run on the player)** | `yoton-index` + its `card` adapters over `yoton-fs` (host-tested on FAT32/exFAT images). Files/Music › Folders sorted (Name/Date/Size, hidden per setting), letter jump from the index's letter runs, play queue in listing order. Library index, tag browsing, playlists, Favourites: not started |
| AU-14 | done (host); **device: Pictures app and album art (built, not yet run on the player)** | `yoton-img`. Pictures (folders + pictures, list mode), viewer (fit/fill, rotate, next/prev, slideshow), album art made at track open and cached in `/.yoton/cache/art`. Not done: thumbnail grid, 100 % + pan, wallpaper, delete, per-file rotation memory. User check 14 |
| OTP-1, BT-1, USB-6 | done (host) | `yoton-otp`, `yoton-bt`, MMC/CD-ROM in `yoton-usb` |
| BT-2 (device) | **built, not yet run on the player** | UART1 driver (`atj2157_hal::uart`, `yoton_bsp::bt`), `yoton_bt::driver`, firmware `bt` task, `BluetoothService`, shutdown 0x24 on power-off. Module off at boot; `bt probe/on/off/status/scan/connect/listen/shutdown/rx/tx` test commands. Not done: volume policy, metadata at track change, pairing UI, U-BT-2 |

## 3. Session log

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
* `yoton-bt`: `driver` (sans-I/O glue: `Port` trait, power on/off,
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
* `yoton-index`: `Spec::images_only` (filter bit 4; index files of the
  Pictures listing are their own `…-n4.idx`; older files still parse).
* `yoton-img` (host-tested): `art::cover_rank` (`cover`, `folder`, `front`,
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
  codec takes the arena: cache lookup (`/.yoton/cache/art`, 52 × 52
  `Axxx.YTC` and 128 × 128 `Bxxx.YTC` buckets) keyed by the track's
  locator, size and mtime → else the embedded front cover (ID3 `APIC`,
  FLAC `PICTURE` via `yoton-meta`; tag reader and tags placed in the
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
* Simulator: `PictureService` decodes for real (`yoton-img`, 64 KiB
  arena) from a second read-only mount of the card; the card has
  `Aurora.png`, `Lake.bmp`, `Sunset.qoi` (generated, `sim/src/pictures.rs`,
  part of the card image's cache key) besides the two damaged stubs.
  22 new golden images (`pictures_*`: list, menus, PNG, BMP, QOI
  letterboxed, rotated, filled, damaged, busy; dark/Latin and light/CJK),
  `media_files_notice_*` now opens `/Images` (pictures open the viewer).
* RAM (`xtask build`, `__stack_bottom`, this worktree): main stack
  23,768 → 21,696 B (−2,072). Resident rodata +1,760 B: the panic-location
  paths of 14 `yoton-img`/`yoton-meta` files (≈ 1.35 KB here; each path is
  42 B shorter in the main checkout, so expect ≈ −1.5 KB there) plus the
  new strings in the merged string section; `.data` +144 (`SERVER`: art
  key, folder art, generation; `PIC_RESULT`), `.bss` +160 (UI future +128,
  picture statics). Audio task future unchanged (6,192 B). Paged: code
  52 → 62 pages (`yoton_img` 45 KB, `Decoder::step` alone 28.5 KB;
  `yoton_meta` 11.5 KB), rodata 14 → 16; slot 626,688 → 724,992 B.
  Deepest new RT paths: art from the cache while playing nests ≈ 2.2 KB +
  file system under `play_file` (the existing browse path ≈ 1 KB);
  `Decoder::step` 776 B + IDCT 352 B. Check the stack high-water mark
  after Big art during playback.
* Not done: thumbnail grid and its cache, 100 % with panning, rotation
  remembered per file, wallpaper, delete, Details screen (a toast),
  animated GIF, unsynchronised ID3 pictures, Vorbis/Opus embedded art,
  art in the mini player.

### 2026-11-24 — AU-12 firmware: the directory index in the card task (worker, no hardware)

* `yoton-index` additions (host-tested): module `card` — `Storage`,
  `DirSource`, `NameSource` over a `yoton-fs` volume. The traits are
  synchronous, `yoton-fs` is async: each call is polled to completion
  with the caller's task `Context` (`card::spin`; the SD driver's timers
  register with the card task's waker). Index files live in
  `/.yoton/index/X/DDDDDDDD-SF.idx` (X = last hex digit of the directory:
  16 shards keep name lookups short with thousands of folders), merge
  scratch in `/.yoton/index/sort-a|b.tmp` (or `sort-c|d.tmp` for a build
  that runs while another is suspended); directories created on first
  use; open files cached per session (`Handles`) and closed at its end.
  `Index::read_letter_runs`, `LetterTable<N>` (letter runs in RAM, 4 B
  each, answers `letter`/`seek_letter` exactly like the index). Tests:
  build in slots that suspend/resume and close their files, a second
  build on the other scratch pair in between, query, find, rebuild after a
  change, `fsck` clean, mounted with the `/.yoton` write prefix (FAT32
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
  the new `yoton-index` files (absolute paths: shorter in the main
  checkout than in a worker's worktree). Paged code +4 pages
  (`yoton_index` 17.4 KB). Stack frames of the new sync paths: build step
  ≈ 0.9 KB + file system, window from the index ≈ 0.9 KB + `Index::rows`
  0.5 KB (served also during playback; check the stack high-water mark).
* Not done: the quick stamp (an index is only used after the counting
  pass confirms it, as before for the on-disk listing); `Index::find` for
  the focus (a linear `position_of` pass instead); several abandoned
  builds in a row leak all but the last partial file until that folder is
  built again; Music library index, playlists, Favourites (rest of AU-12).

### 2026-11-24 — AU-5 firmware: yoton-player in the card task (worker, no hardware)

* The card task (RT executor) owns a `yoton_player::Player` (in the
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
* Files on the card (formats in `yoton_player::image`, host-tested):
  `/.yoton/queue.bin` (magic `YQU1`, count, queue id = CRC-32 of serial and
  locators, serial, 8-byte locators, CRC-32), written when a folder is
  queued; `/.yoton/player.bin` (588 B: resume slots A/B, 64 B each, then the
  bookmarks, 460 B, magic `YBM2`, each value with its own CRC), written in
  place (only the changed span) at pause, stop, track start, card
  hand-over and power-off — never while audio is being output (a card
  write can outlast the PCM ring), so there is no 60-s save while playing.
  Both are loaded after mount; the resume track's title is resolved and
  offered as Music → Resume.
* `yoton-player` changes: getters (`len`, `queue_id`, `ab`, `sleep_timer`),
  bookmarks carry the sample rate (for their time in the list; `Player`
  624 → 752 B), `SleepAfterMs(0)` also cancels "after this track", a
  shuffled `PlayQueue` picks a seed whose order starts at the chosen item
  (so every item plays once), `StoreImage`/`Loading` (resume slots only; the
  bookmarks are encoded from the player when the file is written), queue
  file encode/decode. 18 tests (4 new).
* RAM (`xtask build`, `__stack_bottom`): stack 24,752 → 22,968 B
  (−1,784 B). Of that, +1,796 B is resident MP3 seek code
  (`Mp3Decoder::find` 1,142 B, `record` 166 B, more `parse_head`, rodata),
  resident through the `yoton_codec_mp3::*` rule — it could be paged by
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

### 2026-11-24 — implementation session 3 (lead)

* Merged AU-14 (device): Pictures app and album art. On the player: Home
  shows Pictures; Show hidden → `/.yoton/test`: a 1600×1200 JPEG letterboxed,
  a truncated JPEG shows "Cannot show this picture", BMP and PNG render, Down
  rotates by 90°. Album art on the player: an MP3 with an embedded JPEG cover
  (uploaded to `/.yoton/test/`) shows it in Now Playing's Big art view; the
  first open of a track with new art costs underruns (9 here) while the
  cover is decoded — open item: decode art without stopping the DAC feed, or
  before the previous track ends. The owner's album files showed no art
  (they may have none embedded).
* The "garbled" scrolling text in screenshots was a screenshot artifact: the
  frame buffer is read in 1 KiB pieces while the UI keeps redrawing the
  marquee row. The UI now holds drawing while a screenshot is read (≤ 300 ms);
  four shots of a scrolling row are clean. Still open: "Shuffle all" opens an
  idle Now Playing.
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

* Merged AU-12 firmware (worker): Files lists through `/.yoton/index/`. On
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

* Merged AU-12 host (`yoton-index`, worker) and AU-5 firmware (yoton-player
  driving playback, worker). On the player: Files → folder → OK on track 2
  plays with "2 / 231"; Next → "3 / 231"; OK pauses; `/.yoton/queue.bin`
  and `/.yoton/player.bin` written; after a normal reboot Music offers
  "Resume ‹title›", which continues the same track at the paused position;
  holding Next seeks. Fixed: the DAC kept running during a seek (7
  underruns per seek, close to the too-slow abort); it now stops for the
  seek (3 seeks: 0 underruns).

### 2026-11-24 — implementation session 2, part 5 (lead)

* Upload path for test files (user-approved): developer-channel `PUT`
  request (≤ 4 KiB per transfer, buffer in the idle decoder arena),
  `fcreate`/`fclose` commands, `xtask dev put LOCAL /.yoton/PATH`; writes go
  through yoton-fs (write prefix `/.yoton`). ≈ 50 KB/s. Uploaded Vorbis,
  Opus (1 s and 0.25 s pages) and Speex test files to `/.yoton/test/`.
* Results above; codec clock floors (Vorbis/Opus: High) set by the audio
  task. Found: the output CRC32 (bitwise, per frame) cost a large share of
  48 MHz; now only with `crc on` (MP3 at 48 MHz: ≈ 87 % idle). A `NeedData`
  the full ring cannot satisfy now ends the track instead of spinning. A
  `REQ_PUT` arm compiled as a catch-all binding (missing import) — caught by
  the yoton-usb tests; `xtask build` does not run clippy, the gate does.

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

* Audio hot path resident (`resident.toml`: `yoton_codec_mp3::*` and the
  DSP stages' `process` functions), stream ring 32 → 16 KiB, decoder arena
  72 → 64 KiB (largest measured need: Opus 59,488 B). MP3 at 48 MHz now
  ≈ 80 % idle (was ≈ 53 % with the MP3 cluster, ≈ 0 % before), 0 underruns,
  all codecs linked; `mp3-decode` hot cluster dropped (resident instead).
* After an underrun the PCM push now waits for the next DMA half, so a
  decoder that cannot keep up still yields (the RT executor no longer
  starves thread mode).
* **Open: DSP chain hangs on the player.** With any single stage active
  (bass, crossfeed or EQ) playback stops inside `Chain::process` (breadcrumb
  5) until the watchdog fires; also with all of yoton-dsp resident, so not
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
* Merged worker results: OTP-1, BT-1, USB-6, AU-5 (`yoton-player`), AU-7,
  AU-6, UI-3/UI-4, AU-14, AU-8/AU-9 with the Ogg layers unified.


### 2026-11-24 — AU-8/AU-7 Ogg reconciliation (worker)

* AU-7 (Vorbis, on main) and AU-8 (Opus, Speex) each wrote a `yoton-ogg`.
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
* Code size (thumbv7em, opt-level s, before LTO): `yoton-ogg` 7.7 KB for
  both readers and the bisection (AU-8's alone was 4.8 KB). `AnyDecoder`
  on thumbv7em: 984 B (Vorbis variant; Opus 784 B, Speex 608 B).

### 2026-11-24 — AU-9 Speex (worker)

* `crates/codecs/speex` (`yoton-codec-speex`): libspeex 1.2.1 decoder
  (BSD-3, `LICENSE-XIPH-SPEEX`) in `f32` (float build; no `f64`), NB/WB/UWB,
  every sub-mode, vocoder/null modes, DTX, enhancer on (libspeex default,
  as ffmpeg leaves it), intensity stereo, any frames per packet, native
  8/16/32 kHz output. Uses `yoton-ogg` unchanged.
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

* `crates/codecs/ogg` (`yoton-ogg`): page parsing, CRC (checked before use
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
| 2026-11-24 | Opus vs libopus float, RFC 8251 vectors (24 decodes) | range-exact; RMS ≤ −128.9 dBFS, max 1 LSB | `cargo test -p yoton-codec-opus --release` (`OPUS_VECTORS`) |
| 2026-11-24 | Opus arena high-water stereo / mono | 59,488 B / 42.1 KiB (budget 64 KiB) | `xtask test codecs --filter opus/` |
| 2026-11-24 | Opus code size thumbv7em (before LTO) | 117.6 KB (+ yoton-ogg 4.8 KB) | `xtask test codec-size` |
| 2026-11-24 | Speex vs libspeex (ffmpeg), 31 files | −97 … −120 dBFS RMS, max 5 LSB | `xtask test codecs --filter speex/` |
| 2026-11-24 | Speex arena NB / WB / UWB | 4,872 / 7,768 / 11,048 B (budget 12 KiB) | same |
| 2026-11-24 | Speex code size thumbv7em (before LTO) | 32.9 KB | `xtask test codec-size` |
| 2026-11-24 | yoton-ogg code size thumbv7em (before LTO), both readers + bisection | 7,673 B | `xtask test codec-size` |
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
| 2026-11-24 | Firmware with yoton-player (AU-5 firmware) | resident 80,724 B (was 79,084), bss 122,608 B, main stack 22,968 B (was 24,752), 48 code + 14 rodata pages | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with BT-2 device side (worktree build) | resident +160 B, bss +2,136 B, main stack −2,288 B; `yoton_bt::driver::Driver` 1,328 B | `xtask build`, `llvm-nm` `__stack_bottom` |
| 2026-11-24 | Firmware with Pictures and album art (AU-14 device), worker worktree build | resident 81,444 B (was 79,532), bss 123,160 B (was 123,000), main stack 21,696 B (was 23,768), 62 code + 16 rodata pages (was 52 + 14), slot 724,992 B | `xtask build`, `nm` `__stack_bottom` |


## 5. Open questions for the user

* User checks 7–10 in `docs/USER-CHECKS.md` (picture, keys, charge current,
  tones). 8 and 10 are blocking for M4 and M5.
* Decisions D-1, D-3 … D-8 in `docs/PLAN.md` §5 (defaults assumed); D-2 decided (full layout).
* Answered 2026-11-24: Bluetooth shutdown command 0x24 **is** part of normal
  power-off (BT-2 adds it to the power-off path); the −50 dBFS Opus SILK seek
  tolerance is accepted; test files may be added to the card (under
  `/.yoton/test/`).
