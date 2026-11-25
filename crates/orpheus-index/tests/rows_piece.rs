//! What one listing piece costs on the card (AU-PERF, 2026-11-25: on the
//! player each `job:rows` piece ran 37–40 ms against a 36–38 ms budget,
//! with one stretch of 31 ms): sector reads and directory steps of a
//! firmware-style session (new handle table, the index file opened, rows
//! read, one `open_locator` per row, files closed), on a card shaped like
//! the owner's (a large root folder with `/.orpheus` made after the music,
//! shards of many index files, other `/.orpheus` users that take the
//! path-walk cache's three slots, the sector cache cold from the playback
//! loop's reads).
//!
//! The path walk to `/.orpheus/index/X/NAME` was the 31 ms: up to hundreds
//! of sector reads with the root folder to search, a dozen with the shard
//! to search, against one or two per row. A session given the place where
//! the last one found the file ([`Place`]) reopens it from its directory
//! entry. `measure_rows_piece` prints the table (`--ignored
//! --nocapture`); the `regress_` test bounds a later piece's card
//! operations; the others check that a place of another volume, of a
//! rebuilt index or of a removed one is not trusted.

use std::path::PathBuf;
use std::process::Command;
use std::task::Poll;

use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirCursor, DirEntry, MountOptions, OpenMode, Volume, WalkCache};
use orpheus_index::card::{CardIo, Handles, Mode, Place};
use orpheus_index::{Builder, FileName, Index, Locator, Progress, Row, SortBy, Spec};

type Vol<'c> = Volume<'c, RamDisk>;

const SPEC: Spec = Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: false };
const ROW0: Row = Row {
    loc: Locator { dir: 0, index: 0 },
    size: 0,
    mtime: 0,
    letter: None,
    kind: orpheus_index::EntryKind::Other,
    hidden: false,
};

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

fn image(exfat: bool) -> Option<RamDisk> {
    let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
    if !have(tool) {
        eprintln!("{tool} missing: skipped");
        return None;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-index-rows-piece");
    std::fs::create_dir_all(&dir).unwrap();
    let p =
        dir.join(format!("{}-{:?}.img", if exfat { "exfat" } else { "fat32" }, std::thread::current().id()));
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(64 << 20).unwrap();
    drop(f);
    let mut c = Command::new(tool);
    if exfat {
        c.args(["-q", "-c", "32768"]);
    } else {
        c.args(["-F", "32", "-s", "1"]);
    }
    assert!(c.arg(&p).output().unwrap().status.success());
    let d = RamDisk::from_bytes(std::fs::read(&p).unwrap());
    let _ = std::fs::remove_file(&p);
    Some(d)
}

fn mount<'c>(disk: RamDisk, cache: &'c mut [CacheSlot], walks: &'c mut WalkCache) -> Vol<'c> {
    let opts = MountOptions { read_only: false, write_prefix: None, partition: None };
    let mut v = block_on(Volume::mount(disk, cache, opts)).expect("mount");
    v.set_walk_cache(walks);
    v
}

fn track(i: usize) -> String {
    format!("/Album/{i:03} - Track Title Number {i}.mp3")
}

/// The card: `roots` artist folders in the root (long names), the album
/// folder `/Album` with `tracks` files, `/.orpheus` made after them,
/// `per_shard` other index files in each of the 16 shards, and an art and
/// a library file (other users of the walk cache). Returns the disk and
/// the album's index.
fn card(exfat: bool, roots: usize, tracks: usize, per_shard: usize) -> Option<(RamDisk, Index)> {
    let disk = image(exfat)?;
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut walks = WalkCache::EMPTY;
    let mut vol = mount(disk, &mut cache, &mut walks);
    block_on(async {
        for i in 0..roots {
            vol.create_dir(&format!("/{i:03} Some Artist Name With A Longer Title")).await.unwrap();
        }
        vol.create_dir("/Album").await.unwrap();
        for i in 0..tracks {
            let p = track(i);
            let mut f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
            vol.write(&mut f, p.as_bytes()).await.unwrap();
            vol.close(f).await.unwrap();
        }
        for d in
            ["/.orpheus", "/.orpheus/index", "/.orpheus/cache", "/.orpheus/cache/art", "/.orpheus/library"]
        {
            vol.create_dir(d).await.unwrap();
        }
        for s in 0..16u32 {
            vol.create_dir(&format!("/.orpheus/index/{s:x}")).await.unwrap();
            for k in 0..per_shard as u32 {
                let name = FileName::index(0x1000 + k * 16 + s, SPEC);
                let f =
                    vol.open(&format!("/.orpheus/index/{s:x}/{}", name.as_str()), OpenMode::CreateNew).await;
                vol.close(f.unwrap()).await.unwrap();
            }
        }
        for p in ["/.orpheus/cache/art/A0000001", "/.orpheus/library/lib-a.ydb"] {
            let mut f = vol.open(p, OpenMode::CreateNew).await.unwrap();
            vol.write(&mut f, &[7u8; 2048]).await.unwrap();
            vol.close(f).await.unwrap();
        }
    });
    let ix = build(&mut vol);
    assert_eq!(ix.count() as usize, tracks);
    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    Some((d, ix))
}

fn album(vol: &mut Vol<'_>) -> DirCursor {
    block_on(vol.open_dir("/Album")).unwrap()
}

/// The album's index built (or rebuilt in place) in one writing session.
fn build(vol: &mut Vol<'_>) -> Index {
    let start = album(vol);
    let mut scratch = vec![0u8; 8192];
    let mut h = Handles::new();
    let mut e = DirEntry::new();
    let mut cur = start;
    block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Write { alt: false });
        let mut b = Builder::new(&mut scratch, start.dir_cluster(), SPEC, 0).unwrap();
        let mut src = io.dir(start, &mut cur, &mut e);
        let ix = loop {
            if let Progress::Done(ix) = b.step(&mut io.store(), &mut src, 256).unwrap() {
                break ix;
            }
        };
        io.close_all().unwrap();
        Poll::Ready(ix)
    }))
}

/// Card operations: sector reads from the device and directory slots
/// stepped through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Ops {
    reads: u64,
    steps: u32,
}

fn ops(vol: &mut Vol<'_>) -> Ops {
    let steps = vol.dir_steps();
    Ops { reads: vol.device().reads, steps }
}

fn since(a: Ops, b: Ops) -> Ops {
    Ops { reads: b.reads - a.reads, steps: b.steps - a.steps }
}

/// What a firmware `job:rows` piece did: its first card operation (the
/// index file opened in a new session and the first rows read) and the
/// longest row after it (the row's entry by its locator).
#[derive(Debug)]
struct PieceOps {
    open: Ops,
    row_max: Ops,
}

/// A read-only session as the firmware's `media::with_index`: the index
/// file reopened at `at` if given; `at` becomes where it was found.
fn session<R>(
    vol: &mut Vol<'_>,
    f: FileName,
    at: &mut Option<Place>,
    g: impl FnOnce(&CardIo<'_, '_, '_, RamDisk>) -> R,
) -> R {
    let mut h = Handles::new();
    let mut g = Some(g);
    let kept = *at;
    let (r, p) = block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Read);
        if let Some(k) = kept {
            io.reuse(&f, k);
        }
        let r = (g.take().unwrap())(&io);
        let p = io.place(&f);
        io.close_all().unwrap();
        Poll::Ready((r, p))
    }));
    assert!(h.is_empty());
    *at = p;
    r
}

/// One firmware-style `job:rows` piece (`media::Server::fill_some`): rows
/// `start..start + rows`, four at a time, each row's entry read by its
/// locator; the names read.
fn piece(
    vol: &mut Vol<'_>,
    ix: &Index,
    start: u32,
    rows: u32,
    at: &mut Option<Place>,
) -> (PieceOps, Vec<String>) {
    let mut e = DirEntry::new();
    let mut out = PieceOps { open: Ops::default(), row_max: Ops::default() };
    let mut names = Vec::new();
    session(vol, ix.file(), at, |io| {
        let mut buf = [ROW0; 4];
        let mut n = 0;
        while n < rows {
            let t = io.with_vol(|v, _| ops(v));
            let got = ix.rows(&mut io.store(), start + n, &mut buf).unwrap();
            assert!(got > 0);
            if n == 0 {
                out.open = since(t, io.with_vol(|v, _| ops(v)));
            }
            for r in &buf[..got] {
                let t = io.with_vol(|v, _| ops(v));
                io.with_vol(|v, cx| orpheus_index::card::spin(cx, v.open_locator(r.loc, &mut e))).unwrap();
                names.push(e.name().to_string());
                let d = since(t, io.with_vol(|v, _| ops(v)));
                if d.reads >= out.row_max.reads {
                    out.row_max = d;
                }
                n += 1;
                if n == rows {
                    break;
                }
            }
        }
    });
    (out, names)
}

/// Another `/.orpheus` user between two pieces (album art, the library):
/// its walks take the path-walk cache's slots.
fn other_users(vol: &mut Vol<'_>) {
    for p in ["/.orpheus/cache/art/A0000001", "/.orpheus/library/lib-a.ydb"] {
        let f = block_on(vol.open(p, OpenMode::Read)).unwrap();
        block_on(vol.close(f)).unwrap();
    }
}

/// The playback loop's own reads (track data, FAT) went through the eight
/// sector-cache slots since the last piece, and its path walks (the
/// read-ahead, the library) through the walk cache.
fn between_pieces(vol: &mut Vol<'_>, walks: bool) {
    if walks {
        other_users(vol);
    }
    vol.invalidate_cached(0, u32::MAX);
}

fn expected(start: usize, n: usize) -> Vec<String> {
    (start..start + n).map(|i| track(i).trim_start_matches("/Album/").to_string()).collect()
}

#[test]
#[ignore = "prints measurements"]
fn measure_rows_piece() {
    println!("first operation of a piece (index file opened + 4 rows read) / one row (its entry by locator)");
    for exfat in [false, true] {
        for (roots, per_shard) in [(50, 4), (400, 4), (400, 60), (1000, 60)] {
            let Some((disk, ix)) = card(exfat, roots, 300, per_shard) else { continue };
            let mut cache = vec![CacheSlot::EMPTY; 8];
            let mut walks = WalkCache::EMPTY;
            let mut vol = mount(disk, &mut cache, &mut walks);
            let label = if exfat { "exFAT 32K" } else { "FAT32 512" };
            // By path each time (the old sessions).
            between_pieces(&mut vol, false);
            let (cold, _) = piece(&mut vol, &ix, 0, 8, &mut None);
            let (warm, _) = piece(&mut vol, &ix, 8, 8, &mut None);
            between_pieces(&mut vol, true);
            let (after, _) = piece(&mut vol, &ix, 16, 8, &mut None);
            vol.set_walk_cache(Box::leak(Box::new(WalkCache::EMPTY)));
            between_pieces(&mut vol, false);
            let (lost, _) = piece(&mut vol, &ix, 16, 8, &mut None);
            // From the kept place.
            let mut at = None;
            piece(&mut vol, &ix, 0, 8, &mut at);
            between_pieces(&mut vol, true);
            let (kept, _) = piece(&mut vol, &ix, 8, 8, &mut at);
            vol.set_walk_cache(Box::leak(Box::new(WalkCache::EMPTY)));
            between_pieces(&mut vol, false);
            let (kept_lost, _) = piece(&mut vol, &ix, 16, 8, &mut at);
            println!(
                "{label}, root {roots:4} folders, {per_shard:2} files/shard | by path: walk cache cold {:?}, \
                 warm {:?}, after art+library + cold sectors {:?}, walk cache lost {:?} | \
                 from the place: after art+library + cold sectors {:?}, walk cache lost {:?} | one row {:?}",
                cold.open, warm.open, after.open, lost.open, kept.open, kept_lost.open, warm.row_max
            );
        }
    }
}

/// A later piece of `job:rows` does no path lookup: its first operation
/// (the index file reopened from the place the last piece kept, rows read)
/// costs a few sector reads with the sector cache cold and the walk cache
/// lost, whatever the size of the root folder or the shard; by path it was
/// up to 342 reads and 5,198 directory steps (`measure_rows_piece`).
#[test]
fn regress_later_rows_pieces_do_no_path_lookup() {
    for exfat in [false, true] {
        let Some((disk, ix)) = card(exfat, 400, 300, 60) else { continue };
        let mut cache = vec![CacheSlot::EMPTY; 8];
        let mut walks = WalkCache::EMPTY;
        let mut vol = mount(disk, &mut cache, &mut walks);
        let mut at = None;
        // The first piece finds the file by path (`job:index` does, in the
        // firmware) and keeps its place.
        let (first, names) = piece(&mut vol, &ix, 0, 8, &mut at);
        assert!(at.is_some());
        assert_eq!(names, expected(0, 8));
        assert!(first.open.reads > 100, "{first:?}: the walk searched the root");
        for k in 1..6u32 {
            between_pieces(&mut vol, true);
            if k % 2 == 0 {
                vol.set_walk_cache(Box::leak(Box::new(WalkCache::EMPTY)));
            }
            let (p, names) = piece(&mut vol, &ix, 8 * k, 8, &mut at);
            assert_eq!(names, expected(8 * k as usize, 8), "piece {k}");
            // The entry's directory sectors (FAT chain, the entry set),
            // the data sector of the rows: no directory searched.
            let bound = if exfat { Ops { reads: 3, steps: 4 } } else { Ops { reads: 5, steps: 16 } };
            assert!(
                p.open.reads <= bound.reads && p.open.steps <= bound.steps,
                "{} piece {k}: {:?} > {bound:?}",
                if exfat { "exFAT" } else { "FAT32" },
                p.open
            );
            assert!(p.row_max.reads <= 2, "{p:?}");
        }
    }
}

/// Patch the volume serial in the boot sector (another card with the same
/// files).
fn other_serial(mut d: RamDisk, exfat: bool) -> RamDisk {
    let at = if exfat { 100 } else { 67 };
    for b in &mut d.data[at..at + 4] {
        *b ^= 0x5A;
    }
    if exfat {
        // The boot checksum (sectors 0..=10) fills sector 11.
        let mut sum: u32 = 0;
        for (j, &b) in d.data[..11 * 512].iter().enumerate() {
            if !matches!(j, 106 | 107 | 112) {
                sum = sum.rotate_right(1).wrapping_add(u32::from(b));
            }
        }
        for w in d.data[11 * 512..12 * 512].chunks_exact_mut(4) {
            w.copy_from_slice(&sum.to_le_bytes());
        }
    }
    d
}

/// A place is used only on the volume it was found on, and only while its
/// entry still holds the file: an index rebuilt in place (new clusters,
/// new size) is read whole from its entry; a removed index whose slot
/// another file took is not read; another card is searched by path.
#[test]
fn a_kept_place_is_revalidated() {
    for exfat in [false, true] {
        let Some((disk, ix)) = card(exfat, 60, 40, 8) else { continue };
        let mut cache = vec![CacheSlot::EMPTY; 8];
        let mut walks = WalkCache::EMPTY;
        let mut vol = mount(disk, &mut cache, &mut walks);
        let dir = album(&mut vol).dir_cluster();
        let mut at = None;
        let (_, names) = piece(&mut vol, &ix, 0, 4, &mut at);
        assert_eq!(names, expected(0, 4));
        let p0 = at.expect("a place");

        // Another card (same files, another serial): looked for by path,
        // and the new place is that card's.
        let (d, r) = block_on(vol.unmount());
        r.unwrap();
        let mut cache2 = vec![CacheSlot::EMPTY; 8];
        let mut walks2 = WalkCache::EMPTY;
        let mut other = mount(other_serial(d.clone(), exfat), &mut cache2, &mut walks2);
        let mut at2 = Some(p0);
        let (p, names) = piece(&mut other, &ix, 4, 4, &mut at2);
        assert_eq!(names, expected(4, 4));
        assert!(p.open.steps > 50, "{p:?}: by path");
        assert!(at2.is_some_and(|a| a != p0 && a.locator() == p0.locator()));
        drop(other);
        let mut vol = mount(d, &mut cache, &mut walks);

        // Rebuilt in place after the folder changed: the place still names
        // the file, read with its new size and clusters.
        block_on(async {
            for i in 40..80 {
                let f = vol.open(&track(i), OpenMode::CreateNew).await.unwrap();
                vol.close(f).await.unwrap();
            }
        });
        let ix2 = build(&mut vol);
        assert_eq!(ix2.file(), ix.file());
        assert_eq!(ix2.count(), 80);
        let mut at = Some(p0);
        let opened =
            session(&mut vol, ix.file(), &mut at, |io| Index::open(&mut io.store(), dir, SPEC).unwrap());
        assert_eq!(opened, Some(ix2), "the rebuilt index, from its place");
        assert_eq!(at, Some(p0));
        let (_, names) = piece(&mut vol, &ix2, 60, 8, &mut at);
        assert_eq!(names, expected(60, 8));

        // Removed, and its slot taken by another file: not read through
        // the place; there is no index any more.
        let shard = format!("/.orpheus/index/{:x}", dir & 0xF);
        block_on(vol.remove(&format!("{shard}/{}", ix.file().as_str()))).unwrap();
        let intruder = FileName::index(dir ^ 0x10, SPEC);
        block_on(async {
            let mut f =
                vol.open(&format!("{shard}/{}", intruder.as_str()), OpenMode::CreateNew).await.unwrap();
            vol.write(&mut f, &[0u8; 4096]).await.unwrap();
            vol.close(f).await.unwrap();
        });
        let mut e = DirEntry::new();
        block_on(vol.open_locator(p0.locator(), &mut e)).unwrap();
        assert_eq!(e.name(), intruder.as_str(), "the slot was reused");
        let mut at = Some(p0);
        let opened =
            session(&mut vol, ix.file(), &mut at, |io| Index::open(&mut io.store(), dir, SPEC).unwrap());
        assert_eq!(opened, None);
        assert_eq!(at, None, "nothing kept for a file that is not there");
        let (d, r) = block_on(vol.unmount());
        r.unwrap();
        drop(d);
    }
}
