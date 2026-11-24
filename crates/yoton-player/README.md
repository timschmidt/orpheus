# yoton-player

The playback service of yotonrs (WP AU-5; 04-audio §2.2, §6.3, §6.5): a
synchronous, allocation-free state machine the RT audio task drives.

```text
firmware events ──► Player::handle(event, store, &mut actions) ──► actions for the firmware
 Command(Next) / Opened{..} / ReaderAtEnd / DecoderEnded / Position{frame} / Tick{ms} / ...
                                         Open{item, start_frame, gapless, paused} / Prefetch{item} / Seek / Pause / ...
```

* **Queue**: a length and an identity (`queue_id`, chosen by the firmware,
  e.g. a hash of `/.yoton/queue.bin`). Items are indices; the firmware
  resolves item → `TrackRef` (the 16-byte locator of 04 §6.3: volume
  serial, directory cluster, entry index, name hash) and opens it with
  `Volume::open_locator`, checking `TrackRef::matches(name)`.
  `queue::queue_dir` lists a directory's playable files into a sink.
* **Shuffle**: `Shuffle`, a keyed 4-round Feistel permutation of `0..n`
  with cycle walking — no per-track RAM, exact inverse (previous, jump to
  item), same order after a restart (seed in the resume record). Turning
  shuffle on keeps the current track and draws a new order around it.
* **Repeat**: off / one / all. `Next` ignores repeat-one; a track ending
  respects it.
* **Gapless hand-over**: `ReaderAtEnd` → `Prefetch{next}` (the reader
  continues into the next file); `DecoderEnded` → `Open{next, gapless:
  true}` (re-open the decoder in the same arena — `inspect` then `open`
  — while the PCM ring drains; no fade). Manual changes (`Next`, `Prev`,
  jump, bookmark) are `gapless: false` (fade, flush, fresh start).
* **Failures**: an `OpenFailed`/`DecoderFailed` skips to the next track;
  `Config::max_failures` in a row (or the whole queue) stops.
* **Resume**: `ResumeRecord` (track, frame, queue id/len, position, item,
  shuffle seed, repeat, playing) saved at pause, stop, track change, every
  60 s of playback (`Config::save_every_ms`) and at `Shutdown`, through
  `PlayerStore` into two alternating CRC-protected 64-byte slots: a power
  loss during a write keeps the previous record. `Player::resume(queue_id,
  queue_len, play, out)` reopens the same queue position at the saved frame,
  or opens the track directly (`OpenTrack`) when the queue changed.
* **Bookmarks** (16, oldest dropped), **A–B repeat**, **seek** in ms
  (absolute and relative), **sleep** after a time or at the end of the
  track, **Prev** restarts the track after 3 s.

Sizes: `Player` 624 bytes (16 bookmarks included); no heap, no float,
`#![forbid(unsafe_code)]`, no recursion.

## Tests

`cargo test -p yoton-player`: a simulated firmware (tracks of known length,
a reader running ahead of the decoder, a PCM sink recording what played)
checks gapless play-through (every hand-over `gapless: true`, played ranges
contiguous and complete), shuffle permutations with repeat-all, repeat-one,
next/prev/seek/jump, skipping broken files and stopping when all are
broken, bookmarks/A–B/sleep timers; power loss after a pause resumes at the
exact frame and in the same shuffled order; power loss while playing loses
at most the save interval; a write cut at any byte keeps the previous
record. `tests/dir_queue.rs` builds a queue from a real FAT image
(`mkfs.fat`), re-opens every locator and checks the resume record's track.

Real decoders across track boundaries are checked by the codec suite:
`cargo xtask test codecs` decodes album-style splits back to back in one
72 KiB arena and requires the join to have the source's exact length
(FLAC, WAV: bit-exact; MP3: −119 dBFS against ffmpeg's per-track decodes).
