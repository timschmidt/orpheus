# orpheus-fs

Allocation-free, async FAT12/16/32 and exFAT for the Yoton YM03 firmware
(WP FS-1, spec: `docs/plan/02-architecture.md` §6, tests: `docs/plan/07-tooling-testing.md` §3.1, §3.4).

`#![no_std]`, no `alloc`, `#![forbid(unsafe_code)]`, no recursion, no
dependencies. Malformed media produce `Error::Corrupt` (or another error),
never a panic (fuzzed, see below). Host helpers live behind the `std`
feature.

## Decision: own crate, no third-party file system

Evaluated against the 02 §6 criteria (async without allocation, exFAT,
extent access, bounded RAM ≤ 8 KiB cache + ≤ 256 B per open file, write
support, licence MIT/Apache):

| Candidate | Result |
|---|---|
| `embedded-fatfs` (MIT, git-only; crates.io has a 0.0.0 placeholder) | FAT only, no exFAT; byte-stream I/O through `embedded-io-async` adapters rather than direct multi-sector transfers into the caller's buffer; no extent API; no resumable directory cursor that survives across UI frames (research §5 "could not verify"). Would still need a second crate for exFAT. |
| `exfat-slim` 0.7.0 (Apache-2.0, read 2026-11-23) | exFAT only. Its up-case table is the first 128 code points (ASCII-only case folding); no MBR handling (the boot sector must be at LBA 0); no extent API; no resumable cursor; three separate `N`-slot caches (bitmap, FAT, data) plus a per-file `heapless::Vec` of touched sectors; README TODO: zero-fill past the valid-data length. |
| `block-device-driver` 0.2 (MIT) | Only a trait. Its `Aligned<A, [u8; SIZE]>` buffers cannot be produced from the stream ring / USB buffers without `unsafe` or a copy. |
| `embedded-sdmmc` | Synchronous, FAT16/32 only (research §5). |

Two crates (FAT + exFAT) would also mean two caches, two directory
iterators, two long-name implementations and two code bases in the paged
image. `orpheus-fs` shares the cache, the directory walker, the name matching
and the allocation logic between FAT and exFAT (about 3 300 lines without
tests, tables and host helpers). An adapter from `block-device-driver::BlockDevice` to
`orpheus_fs::BlockDevice` (or the reverse, for the `sdio` crate) is a few
lines if the SD driver ends up speaking that trait.

## API

```rust
pub trait BlockDevice {                    // 512-byte sectors, absolute LBAs
    type Error;
    async fn read(&mut self, lba: u32, buf: &mut [[u8; 512]]) -> Result<(), Self::Error>;
    async fn write(&mut self, lba: u32, buf: &[[u8; 512]]) -> Result<(), Self::Error>;
    async fn sync(&mut self) -> Result<(), Self::Error> { Ok(()) }   // card busy done
    fn sector_count(&self) -> u32;
}
```

* One call = one multi-block transfer (CMD18/CMD25). File data goes straight
  between the device and the caller's buffer whenever the request covers
  whole sectors of a contiguous cluster run; only partial sectors and
  metadata use the cache. Buffers carry no alignment guarantee (bounce in the
  driver if DMA needs it). `&mut T` implements the trait too.
* `Volume::mount(dev, &mut [CacheSlot], MountOptions { read_only, write_prefix, partition })`
  finds an MBR partition (types 01/04/06/07/0B/0C/0E, or the given index) or
  a boot sector at LBA 0 ("superfloppy"); GPT → `Unsupported`.
* Files are plain values (`File`, 64 bytes) and directory cursors
  (`DirCursor`, 48 bytes) do not borrow the volume: put the `Volume` in an
  `embassy_sync::mutex::Mutex` and lock it per call.

| Call | Notes |
|---|---|
| `open(path, OpenMode::{Read, ReadWrite, Create, CreateNew, Truncate})` | Case-insensitive path walk; `.` ignored, `..` refused |
| `read`, `read_exact_or_eof`, `write`, `File::seek/set_position` | Seek is free (no I/O); writing past the end zero-fills the gap |
| `set_len` | FAT: zero-fills; exFAT: allocates only (valid-data length unchanged, reads as zeros) |
| `mark_valid` | exFAT: valid length := size (raw-LBA scratch files) |
| `flush_file` / `close` | Writes the entry (size, first cluster, time, exFAT checksum), then `flush()` — fsync semantics |
| `flush` | All dirty sectors, FSINFO, clears the volume dirty flag, `sync()` |
| `create_dir`, `remove` (file or empty dir), `rename` (also moves; replaces an existing file) | |
| `open_dir`, `root`, `read_dir(&mut cursor, &mut DirEntry) -> bool`, `seek_dir(cursor, index)`, `dir_cursor(&entry)` | Iteration from disk, cursor is `Copy` and resumable |
| `stat`, `exists`, `open_entry(&DirEntry)` (read-only) | |
| `DirEntry::locator()` / `open_locator` | `(dir first cluster, entry index)`: 04-audio §6.3 locators without a path walk |
| `extents(&File)` + `next_extent` → `Extent { lba, sectors, offset }`, `is_contiguous` | Absolute device LBAs; last run cut to the file size |
| `free_clusters`, `free_bytes`, `kind`, `cluster_size`, `serial`, `set_time`, `cache_stats`, `dirty_sectors`, `device()` + `invalidate_cached` (raw-LBA work), `unmount() -> (D, Result)` | |

Names: long names on FAT (UTF-16, with 8.3 aliases `NAME~N` / `NAHHHH~N` and
Windows NT lower-case flags for all-lower-case 8.3 names), exFAT names with
name hash from the volume's up-case table. `DirEntry::name()` is UTF-8 (up to
765 bytes). Lookup uses Unicode simple case folding (CaseFolding status C+S,
whole BMP: Latin, Greek, Cyrillic, Armenian, Georgian, Cherokee, ...);
`unicode::fold`/`eq_ignore_case` are public for the collation of 04 §6.1.
Tables are generated by `tools/gen_tables.py` (Unicode 17 data; the exFAT
recommended up-case table read back from `mkfs.exfat` and checked against
its checksum 0xE619D30D). Regenerate with
`python3 tools/gen_tables.py > src/tables.rs && cargo fmt`.

### Write prefix

`MountOptions::write_prefix = Some("/.orpheus")`: every mutating path call
(`open` for writing, `create_dir`, `remove`, `rename` both ends) must lie at
or below the prefix (component-wise, case-insensitive) or fails with
`OutsidePrefix`; `..` is refused everywhere. Handles from `open_entry` are
read-only. Raw-LBA writes through `device()` are outside this check.

## Write ordering and power cuts

* Cache write-back order: allocation (FAT/bitmap) and data sectors before any
  directory sector, lowest LBA first; FAT copies are written together.
* Allocation: cluster marked used → data written → directory entry updated.
  Freeing (truncate, delete): entry written and flushed → clusters freed.
* `rename` removes the old entry (flushed) before writing the new one: a cut
  in between loses the name, the clusters leak (fsck recovers them); it never
  leaves two entries sharing clusters. A failed rename restores the old entry.
* New FAT directories: `.`/`..` reach the medium before the parent entry.
* Dirty flag: FAT16/32 `FAT[1]` clean bit, exFAT `VolumeFlags.VolumeDirty`,
  set (written through) at the first change, cleared by `flush`.

What a cut can leave (verified by `tests/powercut.rs`, about 260 cut
points over FAT12/16/32/exFAT, torn multi-sector writes included): the volume
always mounts and walks, no two files share a cluster, and `fsck -n` reports
only leak-class findings: lost clusters, a chain longer than the file size
("truncating"), wrong free count, FAT copies differing, orphaned long-name
entries, and on exFAT "more clusters are allocated" for a FAT-chained
directory that grew.

## RAM

Measured on the host (`cargo test -p orpheus-fs -- --nocapture ram`; pointers
are 8 bytes here, 4 on the device, so device figures are at most these):

| Item | Bytes |
|---|---|
| `Volume` state without the device and the cache slots | 148 |
| `WalkCache` (3 directories, lent with `set_walk_cache`) | 140 |
| `CacheSlot` (one sector + tag); minimum 2 slots, 8 planned (4 160 B) | 520 |
| `File` (position, sizes, entry location, one-run extent cache) | 64 |
| `DirCursor` | 48 |
| `DirEntry` (name buffer 768 B + metadata) | 888 |
| Futures: `read` 552, `write` 704, `read_dir` 792, `flush` 352, `set_len` 712, `open` 1 576, `create_dir` 1 680, `remove` 1 120, `rename` 2 024, `mount` 2 064 | (in the calling task's future) |

No static data except read-only tables (case-fold and up-case runs, 317 of
them, about 2.5 KiB; CP437, 256 B) and one 512-byte zero sector.

## Tests

`cargo test -p orpheus-fs` (needs `mkfs.fat`, `fsck.fat`, mtools, `mkfs.exfat`,
`fsck.exfat`, `dump.exfat`; all present on the Fedora host). Images live
under `target/tmp/orpheus-fs/`; every mutating test ends with `fsck -n`.

| Test | What |
|---|---|
| unit (`src/`) | Case folding against `CaseFolding.txt`, up-case, CP437, in-place UTF-16→UTF-8, 8.3 forms and aliases, name validation, RAM figures |
| `write_ops` | FAT12 (s1, s8), FAT16 (s1, s8), FAT32 (s1, s8), exFAT (512 B, 4 KiB, 32 KiB clusters): mkdir, files of edge sizes written in random chunks, overwrite, write past end, truncate/extend, open modes, case-insensitive lookup, renames (same dir, cross dir, case only, over a file, a directory), deletes, free-count check after remount, read back against a model, mtools reads every FAT file identically, `dump.exfat` lists exFAT names |
| `bigdir` | 5 000 long Unicode names on FAT16, FAT32, exFAT: list in 37-entry windows from stored cursors, `seek_dir`, locator round trip, case-insensitive lookups, delete half |
| `large_file` | exFAT file of 4.3 GiB (sparse image), reads across the 4 GiB boundary, one extent, allocate-only extension to 5 GiB; FAT32 refuses ≥ 4 GiB |
| `linux_images` | Files and directories written by mtools (Unicode, 8.3, lower-case 8.3, mixed case) on FAT12/16/32 read back byte-exact; a file fragmented by mtools mapped through `extents` and read raw; read-only mount leaves the image bit-identical |
| `extents` | Interleaved appends fragment two files (FAT16/32, exFAT incl. NoFatChain→FAT-chain conversion); raw extent reads equal file reads; contiguous scratch file written through raw LBAs |
| `powercut` | See above |
| `mount_options` | MBR with two partitions (partition selection, bytes outside partitions untouched), GPT refused, blank media, write prefix, path rules, fixed FAT16 root full, non-standard exFAT up-case table (name hash checked against `dump.exfat`), 2-slot cache |
| `fuzz_smoke` | Stable-toolchain run of the fuzz harness: directed hostile cluster numbers/sizes in every directory entry, then 3 000 random mutations (`ORPHEUS_FS_FUZZ_ITERS` to change); replays `fuzz/regress/*`; writes the fuzz seed images |
| `ram_figures` | Future sizes |

exFAT files created by Linux could not be tested: exfatprogs has no copy
tool, no FUSE exFAT driver is installed, and loop mounts need root. exFAT is
covered by our own round trips, `fsck.exfat` and `dump.exfat`.

### Fuzzing

```
cargo test -p orpheus-fs --test fuzz_smoke          # writes the seed images
cd crates/orpheus-fs && cargo +nightly-2026-11-05 fuzz run mount_walk -- -max_len=4096
```

The input picks one of four seed images (FAT12 128 KiB, FAT16 4 MiB, FAT32
33 MiB, exFAT 3 MiB, each with nested directories, long names and fragmented
files) and patches bytes in its first 16 MiB; the harness mounts (read-only
or read-write), walks and reads everything, performs a few writes, flushes,
remounts and walks again. Crashes go to `fuzz/regress/` once fixed.
(The default `nightly` on this host is a 1.97 nightly, too old for the
workspace's `rust-version`.)

## Not supported / known limits

* Logical sector size other than 512; GPT; exFAT with two FATs (TexFAT) is
  mounted read-only.
* No in-crate formatter (mkfs); volume label not exposed.
* Open handles are not tracked: removing or renaming a file that is open
  leaves a stale handle (callers own that rule).
* A `File` keeps one cached extent run; a fragmented file walks the FAT once
  per fragment on sequential reads (re-walks from the start on backward
  seeks before the cached run).
* 8.3 alias generation tries 8 candidates per name; a directory where all
  are taken returns `DirectoryFull`.
* Case folding: BMP only (supplementary-plane letters compare exactly).
* A non-standard exFAT up-case table is supported but slow (the table is
  scanned per non-ASCII character when hashing a new name).
* exFAT `PercentInUse` is not updated (fsck does not mind); UTC offsets are
  written as "unspecified".
* Indexing panic sites exist where bounds hold by construction; no fuzz run
  has reached one, but there is no static proof.
* Not measured yet: code size in the firmware image, speed on the device.
